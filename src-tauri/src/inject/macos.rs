//! macOS text injection: arboard clipboard + Cmd+V via CGEvent.
//!
//! Pattern adopted from a reference Swift implementation that ships at scale:
//!
//! 1. Snapshot the user's prior clipboard contents.
//! 2. Write the transcript to the clipboard.
//! 3. **Wait for any held modifiers to be released**, up to 600ms.
//!    Otherwise the paste fires while ⌘/⌥/⌃/⇧ are still down from the
//!    dictation hotkey and Cmd+V becomes something else.
//! 4. Wait another 30ms (lets the clipboard settle and gives the OS a
//!    moment between modifier-release and our synthetic event).
//! 5. Post two CGEvents (V keydown + V keyup) with `.maskCommand`,
//!    targeting `.cgSessionEventTap`.
//! 6. After 1s, restore the prior clipboard IF the clipboard still
//!    holds exactly what we wrote (so the user can copy something
//!    new mid-paste and we won't clobber it).
//!
//! Constants tuned by a reference implementation at scale; adopted directly.
//!
//! Known gap (deferred to Phase 7 polish): we don't declare transient
//! pasteboard types yet, so clipboard managers (Raycast, Maccy, Paste)
//! will see and record each dictation. arboard doesn't expose
//! declareTypes; doing this needs a direct NSPasteboard call.

use anyhow::{anyhow, Context, Result};
use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use std::thread;
use std::time::Duration;

/// Mac virtual key codes. Hard-coded for US QWERTY — covers >95% of
/// keyboards. Phase 7 polish can add TIS-based dynamic lookup
/// (UCKeyTranslate + TISCopyCurrentKeyboardInputSource) so Dvorak /
/// AZERTY layouts also work.
const KEYCODE_V: u16 = 9;
const KEYCODE_C: u16 = 8;

/// Modifier keycodes we poll to know whether to wait before posting paste.
/// L/R pairs because the keyboard exposes both physical keys distinctly.
const MODIFIER_KEYCODES: &[u16] = &[
    55, // Command (Left)
    54, // Command (Right)
    56, // Shift (Left)
    60, // Shift (Right)
    58, // Option (Left)
    61, // Option (Right)
    59, // Control (Left)
    62, // Control (Right)
];

/// the reference implementation's tuned constants. We adopt them directly.
const MOD_POLL_INTERVAL: Duration = Duration::from_millis(25);
const MOD_POLL_MAX_ATTEMPTS: u32 = 24; // = 600ms ceiling
const POST_RELEASE_DELAY: Duration = Duration::from_millis(30);
const CLIPBOARD_SETTLE: Duration = Duration::from_millis(40);
const CLIPBOARD_RESTORE_DELAY: Duration = Duration::from_millis(1000);

// CGEventSourceKeyState lives in CoreGraphics but the Rust core-graphics
// crate doesn't bind it. Declare manually. The C signature is:
//   bool CGEventSourceKeyState(CGEventSourceStateID stateID, CGKeyCode key);
// C99 `bool` is ABI-compatible with Rust's `bool` on every platform Apple
// supports. CGKeyCode is u16.
extern "C" {
    fn CGEventSourceKeyState(state_id: CGEventSourceStateID, key: u16) -> bool;
}

pub fn inject_text(text: &str) -> Result<()> {
    if text.is_empty() {
        return Ok(());
    }

    let mut clipboard = arboard::Clipboard::new().context("opening clipboard")?;
    // Best-effort: preserve previous text clipboard content and restore after paste.
    let previous = clipboard.get_text().ok();

    let payload = text.to_string();
    clipboard
        .set_text(payload.clone())
        .context("writing to clipboard")?;

    // Give the OS a moment to settle the clipboard content before pasting.
    thread::sleep(CLIPBOARD_SETTLE);

    wait_for_modifiers_released();

    post_paste().context("posting Cmd+V")?;

    // Drop the paste-time clipboard so the background restore can open
    // its own handle. Safety guard: only restore if the clipboard still
    // holds *our* paste — that way nothing the user copies in between
    // gets overwritten.
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

/// Synthesize Cmd+V at the Session-level event tap.
pub fn send_ctrl_v() -> Result<()> {
    post_command_combo(KEYCODE_V)
}

/// Synthesize Cmd+C at the Session-level event tap.
pub fn send_ctrl_c() -> Result<()> {
    post_command_combo(KEYCODE_C)
}

/// Internal paste. Splits out from `send_ctrl_v` so `inject_text` can
/// call it with its own (clearer) name; behavior is identical.
fn post_paste() -> Result<()> {
    post_command_combo(KEYCODE_V)
}

fn post_command_combo(keycode: u16) -> Result<()> {
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow!("CGEventSource::new failed"))?;

    let key_down = CGEvent::new_keyboard_event(source.clone(), keycode, true)
        .map_err(|_| anyhow!("CGEvent::new_keyboard_event(keyDown) failed"))?;
    key_down.set_flags(CGEventFlags::CGEventFlagCommand);
    key_down.post(CGEventTapLocation::Session);

    let key_up = CGEvent::new_keyboard_event(source, keycode, false)
        .map_err(|_| anyhow!("CGEvent::new_keyboard_event(keyUp) failed"))?;
    key_up.set_flags(CGEventFlags::CGEventFlagCommand);
    key_up.post(CGEventTapLocation::Session);

    Ok(())
}

/// Poll the combined session keyboard state until no modifier key is
/// down, capped at 600ms. Once released, sleep an additional 30ms to
/// give the OS a beat between the last modifier-up event and our
/// synthetic paste.
///
/// If the timeout fires, paste anyway (logged) — better to paste with
/// a stuck modifier than hang.
fn wait_for_modifiers_released() {
    let state = CGEventSourceStateID::CombinedSessionState;
    for _ in 0..MOD_POLL_MAX_ATTEMPTS {
        let any_held = MODIFIER_KEYCODES
            .iter()
            .any(|&k| unsafe { CGEventSourceKeyState(state, k) });
        if !any_held {
            thread::sleep(POST_RELEASE_DELAY);
            return;
        }
        thread::sleep(MOD_POLL_INTERVAL);
    }
    tracing::warn!("modifier-release wait timed out at 600ms; pasting anyway");
}
