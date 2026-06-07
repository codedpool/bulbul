//! Linux text injection: arboard clipboard + Ctrl+V via X11 XTest.
//!
//! Same shape as the Windows pipeline (snapshot clipboard → write →
//! force-release modifiers → synthesize Ctrl+V → restore clipboard),
//! using XTestFakeInput to synthesize the keystrokes instead of
//! SendInput.
//!
//! Pure-Wayland sessions (no XWayland) fall through with an error;
//! Phase 3b will add the libei / portal-based Wayland path. Most modern
//! Wayland desktops (GNOME, KDE, default Ubuntu) ship XWayland by
//! default, so X11 injection covers most users in practice — only a
//! pure-Wayland session without the X11 compatibility layer hits the
//! gap.
//!
//! Keycodes are hard-coded for US QWERTY layout — covers >95% of
//! keyboards. Phase 7 polish can add proper keysym→keycode lookup via
//! XKB to handle Dvorak/AZERTY/etc.

use anyhow::{anyhow, Context, Result};
use std::thread;
use std::time::Duration;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt as XprotoExt, Window, KEY_PRESS_EVENT, KEY_RELEASE_EVENT};
use x11rb::protocol::xtest::ConnectionExt as XTestExt;

// X11 protocol-level keycodes (NOT keysyms). These reflect the physical
// key position on a 104/105-key keyboard, layout-independent for
// modifiers; for the typing keys we assume US QWERTY.
const KC_CONTROL_L: u8 = 37;
const KC_SHIFT_L: u8 = 50;
const KC_SHIFT_R: u8 = 62;
const KC_ALT_L: u8 = 64;
const KC_ALT_R: u8 = 108;
const KC_SUPER_L: u8 = 133;
const KC_SUPER_R: u8 = 134;
const KC_V: u8 = 55;
const KC_C: u8 = 54;

/// Modifiers we force-release before posting Ctrl+V/C. We always release
/// Win/Alt/Shift (Bulbul's hotkey may be holding Win, and Alt/Shift would
/// turn Ctrl+V into a different combo if any were stuck). Ctrl is left
/// alone — we explicitly press it as part of the paste sequence.
const MODS_TO_RELEASE: &[u8] = &[KC_SUPER_L, KC_SUPER_R, KC_ALT_L, KC_ALT_R, KC_SHIFT_L, KC_SHIFT_R];

const CLIPBOARD_SETTLE: Duration = Duration::from_millis(40);
const CLIPBOARD_RESTORE_DELAY: Duration = Duration::from_millis(250);

pub fn inject_text(text: &str) -> Result<()> {
    if text.is_empty() {
        return Ok(());
    }

    let mut clipboard = arboard::Clipboard::new().context("opening clipboard")?;
    let previous = clipboard.get_text().ok();

    let payload = text.to_string();
    clipboard
        .set_text(payload.clone())
        .context("writing to clipboard")?;

    thread::sleep(CLIPBOARD_SETTLE);

    post_ctrl_combo(KC_V).context("posting Ctrl+V")?;

    drop(clipboard);
    if let Some(prev) = previous {
        thread::spawn(move || {
            thread::sleep(CLIPBOARD_RESTORE_DELAY);
            let Ok(mut cb) = arboard::Clipboard::new() else {
                return;
            };
            if let Ok(current) = cb.get_text() {
                if current == payload {
                    let _ = cb.set_text(prev);
                }
            }
        });
    }

    Ok(())
}

pub fn send_ctrl_v() -> Result<()> {
    post_ctrl_combo(KC_V)
}

pub fn send_ctrl_c() -> Result<()> {
    post_ctrl_combo(KC_C)
}

fn post_ctrl_combo(keycode: u8) -> Result<()> {
    let (conn, screen_num) = x11rb::connect(None).context("connecting to X server")?;
    let root = conn
        .setup()
        .roots
        .get(screen_num)
        .ok_or_else(|| anyhow!("no screen {screen_num} on X server"))?
        .root;

    // Force-release any non-Ctrl modifiers the user might be holding from
    // the dictation hotkey (Win/Alt/Shift). If they weren't down, the
    // synthetic release is a no-op at the protocol level.
    for &mod_kc in MODS_TO_RELEASE {
        let _ = conn
            .xtest_fake_input(KEY_RELEASE_EVENT, mod_kc, 0, root, 0, 0, 0)
            .context("releasing modifier")?;
    }

    // Synthesize Ctrl down, key down, key up, Ctrl up.
    conn.xtest_fake_input(KEY_PRESS_EVENT, KC_CONTROL_L, 0, root, 0, 0, 0)
        .context("Ctrl press")?;
    conn.xtest_fake_input(KEY_PRESS_EVENT, keycode, 0, root, 0, 0, 0)
        .context("key press")?;
    conn.xtest_fake_input(KEY_RELEASE_EVENT, keycode, 0, root, 0, 0, 0)
        .context("key release")?;
    conn.xtest_fake_input(KEY_RELEASE_EVENT, KC_CONTROL_L, 0, root, 0, 0, 0)
        .context("Ctrl release")?;

    // x11rb buffers requests; flush ensures they reach the server before
    // we drop the connection at function exit.
    conn.flush().context("flushing X requests")?;
    Ok(())
}
