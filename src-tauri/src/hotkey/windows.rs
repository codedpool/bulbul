//! Windows-native hotkey primitives.
//!
//! Two responsibilities, both Windows-specific:
//!
//! 1. **Modifier-only chord handling** is delegated to the global
//!    `WH_KEYBOARD_LL` hook in `crate::keyboard_hook` — that lives at
//!    the input layer, so chord-modifier keystrokes can be SUPPRESSED
//!    before Windows's shell sees them. The polling watcher this used
//!    to be was unable to prevent Win-key tap detection on release
//!    (Start menu popping mid-dictation) and was also more sensitive
//!    to missed key events / session-lock state corruption. The hook
//!    fixes both.
//!
//! 2. **Release polling for non-modifier-key hotkeys** (e.g.
//!    `Ctrl+Shift+Space`) is still done here. The Tauri global-shortcut
//!    plugin only fires on press; we read raw key state via
//!    `GetAsyncKeyState` on a small thread until the user lets go.
//!
//! The cross-platform shell in `super` (mod.rs) drives this via three
//! calls: `stop_native_watchers`, `spawn_modifier_chord_watcher`, and
//! `spawn_release_poller`.

use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, Instant};

use super::{HotkeyEvent, ParsedHotkey};

/// How often the release poller checks the dictation hotkey's key state.
const RELEASE_POLL_MS: u64 = 25;

/// Convert our key string to a Windows virtual-key code for
/// `GetAsyncKeyState`. Mirrors the keys we accept in `key_name_to_code`
/// (mod.rs) so a hotkey saved in config round-trips between the plugin
/// path (which uses W3C `Code` values) and the release poller path
/// (which needs OS VKs).
fn key_name_to_vk(name: &str) -> Option<i32> {
    let code: i32 = match name {
        "Space" => 0x20,
        "Tab" => 0x09,
        "Return" | "Enter" => 0x0D,
        "Backspace" => 0x08,
        "Escape" => 0x1B,
        "Up" => 0x26,
        "Down" => 0x28,
        "Left" => 0x25,
        "Right" => 0x27,
        "Insert" => 0x2D,
        "Delete" => 0x2E,
        "Home" => 0x24,
        "End" => 0x23,
        "PageUp" => 0x21,
        "PageDown" => 0x22,
        ";" => 0xBA,  // VK_OEM_1
        "'" => 0xDE,  // VK_OEM_7
        "," => 0xBC,  // VK_OEM_COMMA
        "." => 0xBE,  // VK_OEM_PERIOD
        "/" => 0xBF,  // VK_OEM_2
        "\\" => 0xDC, // VK_OEM_5
        "[" => 0xDB,  // VK_OEM_4
        "]" => 0xDD,  // VK_OEM_6
        "-" => 0xBD,  // VK_OEM_MINUS
        "=" => 0xBB,  // VK_OEM_PLUS
        "`" => 0xC0,  // VK_OEM_3
        // Letters: VK_<A-Z> is just ASCII upper.
        x if x.len() == 1 && x.chars().next().unwrap().is_ascii_uppercase() => {
            x.chars().next().unwrap() as i32
        }
        // Digits: VK_0..VK_9 are ASCII '0'..'9'.
        x if x.len() == 1 && x.chars().next().unwrap().is_ascii_digit() => {
            x.chars().next().unwrap() as i32
        }
        // F1..F12 = 0x70..0x7B.
        x if x.starts_with('F') && x[1..].chars().all(|c| c.is_ascii_digit()) => {
            let n: u8 = x[1..].parse().ok()?;
            if (1..=12).contains(&n) {
                0x70 + (n as i32 - 1)
            } else {
                return None;
            }
        }
        _ => return None,
    };
    Some(code)
}

fn is_key_down(vk: i32) -> bool {
    use ::windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
    unsafe { GetAsyncKeyState(vk) < 0 }
}

/// Tear down anything platform-specific from a previous registration.
/// Called by `super::re_register` before installing fresh handlers.
/// On Windows that means clearing the LL keyboard hook's chord mask;
/// any in-flight chord gets a synthetic Released event from the hook so
/// the orchestrator doesn't get stuck holding a recording open.
pub fn stop_native_watchers() {
    crate::keyboard_hook::set_chord_mask(0);
}

/// Register a modifier-only chord (Ctrl+Win, Alt+Win, etc.) with the LL
/// keyboard hook. `tx` is the channel the hook will fire engagement and
/// release events on. The hook itself is installed once at app startup
/// (see `crate::keyboard_hook::install`); this call just publishes the
/// new chord mask. The previous mask was already cleared by
/// `stop_native_watchers` above.
pub fn spawn_modifier_chord_watcher(_tx: Sender<HotkeyEvent>, hotkey: ParsedHotkey) {
    let mask = crate::keyboard_hook::chord_mask_for(&hotkey);
    crate::keyboard_hook::set_chord_mask(mask);
    tracing::info!(
        "registered dictation (LL keyboard hook): {:?} mask=0b{:04b}",
        hotkey,
        mask
    );
}

/// Hold-to-talk release detector for non-modifier-key hotkeys. The
/// global-shortcut plugin only signals key-down; we poll the actual key
/// state to detect when the user lets go. The `release_evt` parameter
/// lets the same poller drive either the dictation pipeline or the
/// polish-dictation pipeline.
pub fn spawn_release_poller(
    tx: Sender<HotkeyEvent>,
    hotkey: ParsedHotkey,
    release_evt: HotkeyEvent,
) {
    use ::windows::Win32::UI::Input::KeyboardAndMouse::{
        VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
    };
    let Some(main_vk) = hotkey.key.as_deref().and_then(key_name_to_vk) else {
        return;
    };
    thread::spawn(move || {
        // Safety net: regardless of polling result, never let this thread
        // outlive a reasonable upper bound on a press. 60 seconds covers
        // even the most generous dictation; if we hit it we fire release
        // anyway so the orchestrator doesn't get stuck.
        let started = Instant::now();
        loop {
            thread::sleep(Duration::from_millis(RELEASE_POLL_MS));
            if started.elapsed() > Duration::from_secs(60) {
                tracing::warn!("release poller timed out — forcing release");
                let _ = tx.send(release_evt.clone());
                return;
            }
            let main_down = is_key_down(main_vk);
            let ctrl_ok = !hotkey.ctrl || is_key_down(VK_CONTROL.0 as i32);
            let shift_ok = !hotkey.shift || is_key_down(VK_SHIFT.0 as i32);
            let alt_ok = !hotkey.alt || is_key_down(VK_MENU.0 as i32);
            let meta_ok = !hotkey.meta
                || is_key_down(VK_LWIN.0 as i32)
                || is_key_down(VK_RWIN.0 as i32);
            if !(main_down && ctrl_ok && shift_ok && alt_ok && meta_ok) {
                let _ = tx.send(release_evt.clone());
                return;
            }
        }
    });
}
