//! Linux text injection.
//!
//! Two paths, picked at runtime:
//!
//! 1. **X11 session** (including XWayland on Wayland desktops that ship
//!    it — GNOME, KDE, default Ubuntu): arboard for clipboard +
//!    x11rb's XTest extension for the synthetic Ctrl+V. Force-releases
//!    Win/Alt/Shift before the paste so a held dictation hotkey
//!    doesn't taint the combo.
//!
//! 2. **Pure-Wayland session** (no X11 backend at all, or KDE Wayland
//!    where XWayland injection is unreliable): shell out to the
//!    standard Wayland CLI tools — `wl-copy` for the clipboard,
//!    `wtype` / `ydotool` for the synthetic Ctrl+V keystroke. The
//!    user must have one of the tools installed; the wrapper picks
//!    whichever is available, in this priority order:
//!
//!        wtype  →  ydotool  →  fallback to X11/XWayland path
//!
//!    Modifier force-release isn't possible on Wayland (the security
//!    model intentionally blocks it). Documented limitation. The
//!    orchestrator's release-of-hotkey → transcription delay
//!    (~300-700ms) means the user's modifiers are physically released
//!    before paste fires in virtually all cases.
//!
//! No new Rust deps for the Wayland path: everything goes through
//! `std::process::Command` invocations of binaries the user already
//! has from their package manager.

use anyhow::{anyhow, Context, Result};
use std::io::Write;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt as XprotoExt, Window, KEY_PRESS_EVENT, KEY_RELEASE_EVENT};
use x11rb::protocol::xtest::ConnectionExt as XTestExt;

// --- X11 protocol-level keycodes (US QWERTY for typing keys) ---
const KC_CONTROL_L: u8 = 37;
const KC_SHIFT_L: u8 = 50;
const KC_SHIFT_R: u8 = 62;
const KC_ALT_L: u8 = 64;
const KC_ALT_R: u8 = 108;
const KC_SUPER_L: u8 = 133;
const KC_SUPER_R: u8 = 134;
const KC_V: u8 = 55;
const KC_C: u8 = 54;
const MODS_TO_RELEASE: &[u8] = &[KC_SUPER_L, KC_SUPER_R, KC_ALT_L, KC_ALT_R, KC_SHIFT_L, KC_SHIFT_R];

// --- ydotool uses Linux input event keycodes (uinput), not X11 keycodes ---
const YDOTOOL_KEY_LEFTCTRL: &str = "29";
const YDOTOOL_KEY_V: &str = "47";
const YDOTOOL_KEY_C: &str = "46";

const CLIPBOARD_SETTLE: Duration = Duration::from_millis(40);
const CLIPBOARD_RESTORE_DELAY: Duration = Duration::from_millis(250);
// Wayland needs more slack than X11: the compositor propagates a
// clipboard change asynchronously (especially through wl-copy's forked
// selection owner), and the paste keystroke — whether via the portal's
// async D-Bus Notify or an external tool — lands a beat after we fire
// it. Under-waiting here is the classic "paste injected nothing" bug.
const WL_CLIPBOARD_SETTLE: Duration = Duration::from_millis(120);
const WL_CLIPBOARD_RESTORE_DELAY: Duration = Duration::from_millis(700);

#[derive(Clone, Copy)]
enum Combo {
    CtrlV,
    CtrlC,
}

impl Combo {
    fn x11_keycode(self) -> u8 {
        match self {
            Combo::CtrlV => KC_V,
            Combo::CtrlC => KC_C,
        }
    }
    fn wtype_key(self) -> &'static str {
        // wtype takes the key name; `v` and `c` are layout-aware.
        match self {
            Combo::CtrlV => "v",
            Combo::CtrlC => "c",
        }
    }
    fn ydotool_key(self) -> &'static str {
        match self {
            Combo::CtrlV => YDOTOOL_KEY_V,
            Combo::CtrlC => YDOTOOL_KEY_C,
        }
    }
}

// Session detection + `which` live in crate::linux_env so the hotkey,
// overlay, and banner code all agree on what kind of session this is.
// The X11 path still works on most Wayland desktops via XWayland, but
// we prefer native Wayland tools when installed — pure-Wayland apps
// can't receive XWayland-injected keystrokes.
use crate::linux_env::{is_wayland, which};

/// The apt/dnf one-liner for the tool that actually works on this
/// desktop. Mutter (GNOME) doesn't implement the virtual-keyboard
/// protocol wtype needs, so GNOME users get pointed at ydotool
/// (uinput-based — works everywhere, needs its daemon enabled).
fn tool_install_hint() -> String {
    if crate::linux_env::is_gnome() {
        "GNOME's compositor doesn't support wtype — install ydotool instead \
         (sudo apt install ydotool, then enable it: \
         systemctl --user enable --now ydotool.service) \
         or log into an \"Ubuntu on Xorg\" session."
            .to_string()
    } else {
        "Install wtype (sudo apt install wtype) — or ydotool if your \
         compositor lacks the virtual-keyboard protocol."
            .to_string()
    }
}

pub fn inject_text(text: &str) -> Result<()> {
    if text.is_empty() {
        return Ok(());
    }

    // On Wayland, always take the Wayland path: its clipboard write
    // (wl-copy) is the one that survives Bulbul being unfocused, and
    // its keystroke chain already ends in an XWayland last resort —
    // see inject_text_wayland. Re-running the full X11 path out here
    // would rewrite the clipboard via arboard and clobber the good
    // wl-copy selection, so we don't.
    if is_wayland() {
        inject_text_wayland(text)
    } else {
        inject_text_x11(text)
    }
}

pub fn send_ctrl_v() -> Result<()> {
    if is_wayland() {
        send_combo_wayland(Combo::CtrlV).or_else(|e| {
            tracing::warn!("Wayland Ctrl+V failed, trying XWayland: {e:#}");
            post_x11_combo(Combo::CtrlV)
        })
    } else {
        post_x11_combo(Combo::CtrlV)
    }
}

pub fn send_ctrl_c() -> Result<()> {
    if is_wayland() {
        send_combo_wayland(Combo::CtrlC).or_else(|e| {
            tracing::warn!("Wayland Ctrl+C failed, trying XWayland: {e:#}");
            post_x11_combo(Combo::CtrlC)
        })
    } else {
        post_x11_combo(Combo::CtrlC)
    }
}

// === Wayland path =========================================================

enum WaylandTool {
    Wtype,
    Ydotool,
}

fn wayland_keystroke_tool() -> Option<WaylandTool> {
    // Mutter (GNOME) doesn't implement the virtual-keyboard protocol
    // wtype needs — it "succeeds" while typing nothing, or errors.
    // Skip it there so an installed wtype can't shadow ydotool/XWayland.
    // ydotool only counts when its daemon socket exists: the client
    // hard-errors without ydotoold, which was surfacing as a paste
    // failure on installs where the package landed via Recommends but
    // the service was never enabled.
    if which("wtype") && !crate::linux_env::is_gnome() {
        Some(WaylandTool::Wtype)
    } else if crate::linux_env::ydotool_ready() {
        Some(WaylandTool::Ydotool)
    } else {
        None
    }
}

fn inject_text_wayland(text: &str) -> Result<()> {
    // Clipboard via wl-copy if available, else fall back to arboard.
    // This choice matters more than it looks: wl-copy forks a process
    // that *holds* the Wayland selection, so the paste target can read
    // it even though Bulbul's own window isn't focused. arboard sets and
    // returns — and on Wayland a background app frequently loses the
    // selection immediately, so the subsequent Ctrl+V pastes stale or
    // empty content. That's the #1 cause of "dictation recorded but
    // nothing got typed." We depend on wl-clipboard in the .deb/.rpm so
    // this path is the norm; the arboard branch is a last resort and we
    // log loudly when we're on it.
    let use_wl_copy = which("wl-copy") && which("wl-paste");
    if !use_wl_copy {
        tracing::warn!(
            "wl-copy not found — using arboard, which is unreliable for \
             background paste on Wayland. Install wl-clipboard for a fix."
        );
    }

    let previous = if use_wl_copy {
        wl_paste_read().ok()
    } else {
        arboard::Clipboard::new()
            .ok()
            .and_then(|mut c| c.get_text().ok())
    };

    let payload = text.to_string();
    if use_wl_copy {
        wl_copy_write(&payload).context("wl-copy write")?;
    } else {
        let mut clipboard = arboard::Clipboard::new().context("opening arboard clipboard")?;
        clipboard.set_text(payload.clone()).context("arboard write")?;
    }

    thread::sleep(WL_CLIPBOARD_SETTLE);

    // Keystroke chain: portal → tools → XWayland XTest. The clipboard is
    // already set (above) and XWayland apps read the bridged Wayland
    // selection, so the XTest last resort only fires the combo — no
    // clipboard rewrite.
    send_combo_wayland(Combo::CtrlV)
        .or_else(|e| {
            tracing::warn!("Wayland Ctrl+V failed, trying XWayland XTest: {e:#}");
            post_x11_combo(Combo::CtrlV)
        })
        .map_err(|e| {
            anyhow!(
                "couldn't deliver the paste keystroke on any path ({e}). {}",
                tool_install_hint()
            )
        })?;

    // Background restore. Only restore if the clipboard still holds
    // our paste — same guard as the X11 path so a user-initiated
    // mid-paste copy isn't clobbered.
    if let Some(prev) = previous {
        let payload_for_restore = payload.clone();
        let use_wl_copy_for_restore = use_wl_copy;
        thread::spawn(move || {
            thread::sleep(WL_CLIPBOARD_RESTORE_DELAY);
            let current = if use_wl_copy_for_restore {
                wl_paste_read().ok()
            } else {
                arboard::Clipboard::new()
                    .ok()
                    .and_then(|mut c| c.get_text().ok())
            };
            if current.as_deref() == Some(payload_for_restore.as_str()) {
                if use_wl_copy_for_restore {
                    let _ = wl_copy_write(&prev);
                } else if let Ok(mut cb) = arboard::Clipboard::new() {
                    let _ = cb.set_text(prev);
                }
            }
        });
    }

    Ok(())
}

fn send_combo_wayland(combo: Combo) -> Result<()> {
    // Give the user's physical modifiers a moment to clear. The stop
    // press is Ctrl+Alt+Space — if our synthetic Ctrl+V fires while Alt
    // is still physically held, the app receives Ctrl+Alt+V and pastes
    // nothing. macOS polls key state for this (inject/macos.rs step 3);
    // Wayland has no global key query, so a fixed drain is the best we
    // can do. Transcription latency usually covers it; this covers the
    // fast-transcript case.
    thread::sleep(Duration::from_millis(150));

    // Portal first — native, no external tool, and asking while it's
    // down is what triggers its self-healing re-init (so a user who
    // approves the permission dialog late still converges to the portal
    // without restarting Bulbul). Any error falls through to the tool
    // path below.
    let key = match combo {
        Combo::CtrlV => super::linux_portal_paste::EV_V,
        Combo::CtrlC => super::linux_portal_paste::EV_C,
    };
    match super::linux_portal_paste::send_combo(key) {
        Ok(()) => return Ok(()),
        Err(e) => tracing::warn!("portal paste failed, trying tools: {e}"),
    }
    match wayland_keystroke_tool() {
        Some(WaylandTool::Wtype) => {
            // wtype -M ctrl -k v   (press Ctrl modifier, tap key, release)
            let status = Command::new("wtype")
                .args(["-M", "ctrl", "-k", combo.wtype_key()])
                .status()
                .context("running wtype")?;
            if !status.success() {
                return Err(anyhow!("wtype exited with status {status}"));
            }
            Ok(())
        }
        Some(WaylandTool::Ydotool) => {
            // ydotool uses uinput-level keycodes with format <code>:<state>.
            // 1 = pressed, 0 = released.
            let args = [
                "key".to_string(),
                format!("{YDOTOOL_KEY_LEFTCTRL}:1"),
                format!("{}:1", combo.ydotool_key()),
                format!("{}:0", combo.ydotool_key()),
                format!("{YDOTOOL_KEY_LEFTCTRL}:0"),
            ];
            let status = Command::new("ydotool")
                .args(&args)
                .status()
                .context("running ydotool")?;
            if !status.success() {
                return Err(anyhow!("ydotool exited with status {status}"));
            }
            Ok(())
        }
        None => Err(anyhow!(
            "no Wayland keystroke tool available. {}",
            tool_install_hint()
        )),
    }
}

fn wl_copy_write(text: &str) -> Result<()> {
    let mut child = Command::new("wl-copy")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("spawning wl-copy")?;
    {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| anyhow!("wl-copy stdin unavailable"))?;
        stdin.write_all(text.as_bytes()).context("writing to wl-copy stdin")?;
    }
    let status = child.wait().context("waiting on wl-copy")?;
    if !status.success() {
        return Err(anyhow!("wl-copy exited with status {status}"));
    }
    Ok(())
}

fn wl_paste_read() -> Result<String> {
    let output = Command::new("wl-paste")
        .arg("--no-newline")
        .output()
        .context("running wl-paste")?;
    if !output.status.success() {
        return Err(anyhow!("wl-paste exited with status {}", output.status));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

// === X11 path =============================================================

fn inject_text_x11(text: &str) -> Result<()> {
    let mut clipboard = arboard::Clipboard::new().context("opening clipboard")?;
    let previous = clipboard.get_text().ok();

    let payload = text.to_string();
    clipboard
        .set_text(payload.clone())
        .context("writing to clipboard")?;

    thread::sleep(CLIPBOARD_SETTLE);

    post_x11_combo(Combo::CtrlV).context("posting Ctrl+V")?;

    drop(clipboard);
    if let Some(prev) = previous {
        thread::spawn(move || {
            thread::sleep(CLIPBOARD_RESTORE_DELAY);
            let Ok(mut cb) = arboard::Clipboard::new() else { return };
            if let Ok(current) = cb.get_text() {
                if current == payload {
                    let _ = cb.set_text(prev);
                }
            }
        });
    }
    Ok(())
}

fn post_x11_combo(combo: Combo) -> Result<()> {
    let (conn, screen_num) = x11rb::connect(None).context("connecting to X server")?;
    let root: Window = conn
        .setup()
        .roots
        .get(screen_num)
        .ok_or_else(|| anyhow!("no screen {screen_num} on X server"))?
        .root;

    // Force-release Win/Alt/Shift (the dictation hotkey may have any of
    // them held). Ctrl is left alone — we explicitly press it as part
    // of the combo.
    for &mod_kc in MODS_TO_RELEASE {
        let _ = conn
            .xtest_fake_input(KEY_RELEASE_EVENT, mod_kc, 0, root, 0, 0, 0)
            .context("releasing modifier")?;
    }

    let keycode = combo.x11_keycode();
    conn.xtest_fake_input(KEY_PRESS_EVENT, KC_CONTROL_L, 0, root, 0, 0, 0)
        .context("Ctrl press")?;
    conn.xtest_fake_input(KEY_PRESS_EVENT, keycode, 0, root, 0, 0, 0)
        .context("key press")?;
    conn.xtest_fake_input(KEY_RELEASE_EVENT, keycode, 0, root, 0, 0, 0)
        .context("key release")?;
    conn.xtest_fake_input(KEY_RELEASE_EVENT, KC_CONTROL_L, 0, root, 0, 0, 0)
        .context("Ctrl release")?;

    conn.flush().context("flushing X requests")?;
    Ok(())
}
