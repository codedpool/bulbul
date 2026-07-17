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
// X11 selection ownership is IN-PROCESS: whoever called set_text must stay
// alive to answer the target's SelectionRequest. Ctrl+V is asynchronous —
// the app gets the key, THEN asks us for the text — so we have to keep
// owning the selection across that round-trip. Dropping the clipboard right
// after posting the combo (what we used to do) released ownership before the
// request arrived, and the paste silently got nothing on every X11 session.
// The Wayland path doesn't need this because wl-copy forks its own process
// to serve the selection.
const CLIPBOARD_POST_COMBO: Duration = Duration::from_millis(150);
// Wayland paste timing. A short settle lets wl-copy's forked selection
// owner take hold before we fire the combo; a short post-combo wait lets
// the target app consume the paste before we restore the prior clipboard
// inline. Kept tight on purpose — a long/delayed restore reads as a
// screen flicker.
const WL_CLIPBOARD_SETTLE: Duration = Duration::from_millis(60);
const WL_POST_COMBO: Duration = Duration::from_millis(90);

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

    // Fast path (any session, when uinput access was granted): type the
    // characters straight through the kernel virtual keyboard. This never
    // touches the clipboard, so there's no save → paste → restore
    // round-trip — which is the source of the paste "flicker" — and no
    // dependency on wl-clipboard. It only handles text that maps to a US
    // layout; anything else (emoji, accents) returns Err and we fall
    // through to the clipboard path below, which is layout/Unicode safe.
    if super::linux_uinput::is_ready() {
        match super::linux_uinput::type_text(text) {
            Ok(()) => return Ok(()),
            Err(e) => {
                tracing::info!("uinput direct typing unavailable ({e}); using clipboard paste");
            }
        }
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

/// uinput first, in EVERY session — not just Wayland. The kernel virtual
/// keyboard injects below the X server/compositor, so it lands in every app,
/// and it is already the primary path for typing (see `inject_text`, which
/// tries it regardless of session). Sending the combo straight to XTEST on
/// X11 is why transforms' Ctrl+C never fired even while uinput was live and
/// happily typing dictations into the same apps: `inject_text` used uinput,
/// these helpers didn't. Returns true when the combo was delivered.
fn try_uinput_combo(combo: Combo) -> bool {
    if !super::linux_uinput::is_ready() {
        return false;
    }
    let kc = match combo {
        Combo::CtrlV => super::linux_uinput::KEY_V,
        Combo::CtrlC => super::linux_uinput::KEY_C,
    };
    match super::linux_uinput::send_combo(kc) {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!("uinput combo failed; falling back to the session path: {e}");
            false
        }
    }
}

pub fn send_ctrl_v() -> Result<()> {
    if try_uinput_combo(Combo::CtrlV) {
        return Ok(());
    }
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
    if try_uinput_combo(Combo::CtrlC) {
        return Ok(());
    }
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

    // A short settle before the combo, a short wait after for the app to
    // consume the paste, then restore the clipboard SYNCHRONOUSLY. We
    // used to restore 700ms later on a background thread — that delayed
    // second clipboard change is what read as a "screen refresh a beat
    // after the text lands." Doing it fast and inline blurs it into the
    // paste.
    thread::sleep(WL_CLIPBOARD_SETTLE);

    // Keystroke chain: uinput → portal → tools → XWayland XTest. The
    // clipboard is already set, and XWayland apps read the bridged
    // Wayland selection, so the XTest last resort only fires the combo.
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

    thread::sleep(WL_POST_COMBO);

    // Restore the prior clipboard inline. No re-read guard / no delay —
    // the window between our write and this restore is ~150ms, and
    // keeping it inline avoids the extra wl-paste spawn and the
    // delayed-flicker.
    if let Some(prev) = previous {
        if use_wl_copy {
            let _ = wl_copy_write(&prev);
        } else if let Ok(mut cb) = arboard::Clipboard::new() {
            let _ = cb.set_text(prev);
        }
    }

    Ok(())
}

fn send_combo_wayland(combo: Combo) -> Result<()> {
    // No modifier-drain sleep: with evdev hold-to-talk the paste only
    // fires after the user has released the hotkey (release is what
    // ends the recording), so their physical modifiers are already up
    // by the time we get here. The old 150ms drain was dead latency.

    // uinput first — the kernel virtual keyboard is the only path Mutter
    // can't drop, so it's the reliable default when access is granted
    // (setgid .deb install). Works identically on KDE/wlroots/X11.
    if super::linux_uinput::is_ready() {
        let kc = match combo {
            Combo::CtrlV => super::linux_uinput::KEY_V,
            Combo::CtrlC => super::linux_uinput::KEY_C,
        };
        match super::linux_uinput::send_combo(kc) {
            Ok(()) => return Ok(()),
            Err(e) => tracing::warn!("uinput paste failed, trying portal: {e}"),
        }
    }

    // Portal next — native, no privilege. Asking while it's down is what
    // triggers its self-healing re-init, so a user who approves the
    // permission dialog late still converges without restarting Bulbul.
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

// --- Wayland clipboard access for callers outside this module (the
// transform pipeline). arboard can't reliably read a selection another
// app just copied on Wayland — wl-paste/wl-copy go through the same path
// dictation already uses. ---

/// True when clipboard access should go through wl-copy/wl-paste: a
/// Wayland session with wl-clipboard installed (a .deb/.rpm dependency).
pub fn wayland_clipboard_available() -> bool {
    is_wayland() && which("wl-copy") && which("wl-paste")
}

/// Read the clipboard via wl-paste. `None` when empty or unreadable.
pub fn wayland_clipboard_read() -> Option<String> {
    wl_paste_read().ok().filter(|s| !s.is_empty())
}

/// Read the PRIMARY selection (the currently highlighted text) via
/// wl-paste --primary. Set whenever text is highlighted — by mouse drag
/// or keyboard selection like Ctrl+A — so we can capture a selection
/// without simulating Ctrl+C (whose copy gets corrupted when the
/// transform hotkey's own modifier is still held). `None` when empty.
pub fn wayland_primary_read() -> Option<String> {
    let output = Command::new("wl-paste")
        .args(["--primary", "--no-newline"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&output.stdout).into_owned();
    (!s.is_empty()).then_some(s)
}

/// The X11 twin of `wayland_primary_read`. PRIMARY is an X11 invention that
/// Wayland later copied: it holds whatever is currently highlighted, updated
/// the instant the user selects text — no Ctrl+C, no clipboard clobbering, no
/// synthetic keystroke.
///
/// The transform pipeline read PRIMARY on Wayland but was hardcoded to None on
/// X11, so X11 fell back to the fragile clear-clipboard → send Ctrl+C → read
/// dance. That fallback silently captured nothing on Mint/Cinnamon and every
/// transform failed with "no selection captured" — while the very same
/// transform worked on Wayland, purely because Wayland took this path instead.
pub fn x11_primary_read() -> Option<String> {
    use arboard::{Clipboard, GetExtLinux, LinuxClipboardKind};
    let mut cb = Clipboard::new().ok()?;
    let s = cb
        .get()
        .clipboard(LinuxClipboardKind::Primary)
        .text()
        .ok()?;
    (!s.is_empty()).then_some(s)
}

/// Write text to the clipboard via wl-copy. Best-effort.
pub fn wayland_clipboard_write(text: &str) -> Result<()> {
    wl_copy_write(text)
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

    // Hold selection ownership while the target fetches the text — see
    // CLIPBOARD_POST_COMBO. Dropping here immediately is what made
    // auto-typing silently fail on X11.
    thread::sleep(CLIPBOARD_POST_COMBO);

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

    // Negotiate XTEST explicitly. The server rejects fake-input from a client
    // that hasn't queried the extension, and this surfaces "XTEST missing" as
    // a real error rather than four silently-dropped requests.
    conn.xtest_get_version(2, 2)
        .context("querying XTEST")?
        .reply()
        .context("XTEST extension unavailable on this X server")?;

    // Force-release Win/Alt/Shift (the dictation hotkey may have any of
    // them held). Ctrl is left alone — we explicitly press it as part
    // of the combo.
    for &mod_kc in MODS_TO_RELEASE {
        let _ = conn
            .xtest_fake_input(KEY_RELEASE_EVENT, mod_kc, 0, root, 0, 0, 0)
            .context("releasing modifier")?
            .check();
    }

    // Every fake_input is .check()ed rather than fired and forgotten.
    // xtest_fake_input returns a VoidCookie, and X reports errors for void
    // requests ASYNCHRONOUSLY — so `?` on the cookie alone only catches a
    // failure to write the bytes, never the server rejecting the request. The
    // old code flushed and dropped the connection immediately, so a
    // server-side rejection (or the requests never being processed before we
    // disconnected) was completely invisible: send_ctrl_c() returned Ok and
    // nothing ever reached the target app. .check() round-trips, which both
    // surfaces the real error AND guarantees the server processed each event
    // before we move on (incidentally spacing them, as xdotool does).
    let keycode = combo.x11_keycode();
    conn.xtest_fake_input(KEY_PRESS_EVENT, KC_CONTROL_L, 0, root, 0, 0, 0)
        .context("Ctrl press")?
        .check()
        .context("X server rejected the Ctrl press")?;
    conn.xtest_fake_input(KEY_PRESS_EVENT, keycode, 0, root, 0, 0, 0)
        .context("key press")?
        .check()
        .context("X server rejected the key press")?;
    conn.xtest_fake_input(KEY_RELEASE_EVENT, keycode, 0, root, 0, 0, 0)
        .context("key release")?
        .check()
        .context("X server rejected the key release")?;
    conn.xtest_fake_input(KEY_RELEASE_EVENT, KC_CONTROL_L, 0, root, 0, 0, 0)
        .context("Ctrl release")?
        .check()
        .context("X server rejected the Ctrl release")?;

    Ok(())
}
