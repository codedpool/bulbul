use anyhow::{Context, Result};
use std::thread;
use std::time::Duration;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
    VIRTUAL_KEY, VK_C, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT, VK_V,
};

/// Inject text into the focused application via clipboard + Ctrl+V.
/// This is more reliable than per-character SendInput across web apps,
/// autocomplete-aware editors (VS Code, Notion), and rich-text fields.
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
    thread::sleep(Duration::from_millis(40));

    send_ctrl_v().context("sending Ctrl+V")?;

    // Restore previous clipboard contents in the background so the caller
    // (and the perf log) don't block on the 150ms settle window. Safety
    // guard: only restore if the clipboard still holds *our* paste — that
    // way a user who copied something new in the meantime (or a second
    // dictation that fired before we finished) wins. We never overwrite
    // user-initiated clipboard content.
    drop(clipboard);
    if let Some(prev) = previous {
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(150));
            let Ok(mut cb) = arboard::Clipboard::new() else {
                return;
            };
            match cb.get_text() {
                Ok(current) if current == payload => {
                    let _ = cb.set_text(prev);
                }
                _ => {
                    // Clipboard was touched by user or another flow — leave
                    // their content alone.
                }
            }
        });
    }

    Ok(())
}

pub fn send_ctrl_v() -> Result<()> {
    send_ctrl_combo(VK_V)
}

/// Simulate Ctrl+C so the OS copies the current selection into the clipboard.
pub fn send_ctrl_c() -> Result<()> {
    send_ctrl_combo(VK_C)
}

fn send_ctrl_combo(vk: VIRTUAL_KEY) -> Result<()> {
    // Force-release any modifier keys the user is still holding from the
    // hotkey that triggered us (e.g. Win+Alt+1). Without this, the OS sees
    // Win+Alt+Ctrl+C and the foreground app doesn't interpret that as copy.
    let mut inputs = [
        key_input(VK_LWIN, true),
        key_input(VK_RWIN, true),
        key_input(VK_MENU, true),
        key_input(VK_SHIFT, true),
        key_input(VK_CONTROL, false),
        key_input(vk, false),
        key_input(vk, true),
        key_input(VK_CONTROL, true),
    ];
    let sent = unsafe { SendInput(&mut inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent as usize != inputs.len() {
        return Err(anyhow::anyhow!(
            "SendInput delivered {sent}/{} events",
            inputs.len()
        ));
    }
    Ok(())
}

fn key_input(vk: VIRTUAL_KEY, key_up: bool) -> INPUT {
    let flags: KEYBD_EVENT_FLAGS = if key_up {
        KEYEVENTF_KEYUP
    } else {
        KEYBD_EVENT_FLAGS(0)
    };
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}
