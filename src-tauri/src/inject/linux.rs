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

/// Can we inject natively on this Wayland session? True when the
/// RemoteDesktop portal is live (no tool needed) OR a wtype/ydotool
/// binary is installed. When false, only the XWayland best-effort path
/// remains.
fn wayland_can_inject() -> bool {
    super::linux_portal_paste::is_ready() || wayland_keystroke_tool().is_some()
}

pub fn inject_text(text: &str) -> Result<()> {
    if text.is_empty() {
        return Ok(());
    }

    // On Wayland, prefer the native path — RemoteDesktop portal first,
    // then wtype/ydotool. Otherwise fall through to X11, which only
    // reaches XWayland windows, so if even that connection fails the
    // user has no injection path at all and the error must say what to
    // install, not just "connection refused".
    if is_wayland() {
        if wayland_can_inject() {
            inject_text_wayland(text)
        } else {
            inject_text_x11(text).map_err(|e| {
                anyhow!(
                    "No native Wayland input path is available and the \
                     XWayland fallback failed ({e}). {}",
                    tool_install_hint()
                )
            })
        }
    } else {
        inject_text_x11(text)
    }
}

pub fn send_ctrl_v() -> Result<()> {
    if is_wayland() && wayland_can_inject() {
        send_combo_wayland(Combo::CtrlV)
    } else {
        post_x11_combo(Combo::CtrlV)
    }
}

pub fn send_ctrl_c() -> Result<()> {
    if is_wayland() && wayland_can_inject() {
        send_combo_wayland(Combo::CtrlC)
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
    if which("wtype") {
        Some(WaylandTool::Wtype)
    } else if which("ydotool") {
        Some(WaylandTool::Ydotool)
    } else {
        None
    }
}

fn inject_text_wayland(text: &str) -> Result<()> {
    // Clipboard via wl-copy if available, else fall back to arboard.
    // arboard supports Wayland but has reported timing/encoding edge
    // cases (esp. with umlauts); wl-copy is the canonical tool.
    let use_wl_copy = which("wl-copy") && which("wl-paste");

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

    thread::sleep(CLIPBOARD_SETTLE);

    send_combo_wayland(Combo::CtrlV).context("posting Ctrl+V via Wayland tool")?;

    // Background restore. Only restore if the clipboard still holds
    // our paste — same guard as the X11 path so a user-initiated
    // mid-paste copy isn't clobbered.
    if let Some(prev) = previous {
        let payload_for_restore = payload.clone();
        let use_wl_copy_for_restore = use_wl_copy;
        thread::spawn(move || {
            thread::sleep(CLIPBOARD_RESTORE_DELAY);
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
    // RemoteDesktop portal first — native, no external tool. Any error
    // (revoked grant, timeout) falls through to the tool path below.
    if super::linux_portal_paste::is_ready() {
        let key = match combo {
            Combo::CtrlV => super::linux_portal_paste::EV_V,
            Combo::CtrlC => super::linux_portal_paste::EV_C,
        };
        match super::linux_portal_paste::send_combo(key) {
            Ok(()) => return Ok(()),
            Err(e) => tracing::warn!("portal paste failed, trying tools: {e}"),
        }
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
