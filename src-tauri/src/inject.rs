use anyhow::{Context, Result};
use std::thread;
use std::time::Duration;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
    VIRTUAL_KEY, VK_C, VK_CONTROL, VK_V,
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

    clipboard
        .set_text(text.to_string())
        .context("writing to clipboard")?;

    // Give the OS a moment to settle the clipboard content before pasting.
    thread::sleep(Duration::from_millis(40));

    send_ctrl_v().context("sending Ctrl+V")?;

    // Restore previous clipboard contents after a short delay so the paste lands first.
    if let Some(prev) = previous {
        thread::sleep(Duration::from_millis(150));
        let _ = clipboard.set_text(prev);
    }

    Ok(())
}

fn send_ctrl_v() -> Result<()> {
    send_ctrl_combo(VK_V)
}

/// Simulate Ctrl+C so the OS copies the current selection into the clipboard.
pub fn send_ctrl_c() -> Result<()> {
    send_ctrl_combo(VK_C)
}

fn send_ctrl_combo(vk: VIRTUAL_KEY) -> Result<()> {
    let mut inputs = [
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
