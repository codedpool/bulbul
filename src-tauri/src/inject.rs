use anyhow::{Context, Result};
use std::thread;
use std::time::Duration;

/// Inject text into the focused application via clipboard + paste shortcut.
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

    send_paste().context("sending paste shortcut")?;

    // Restore previous clipboard contents in the background.
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
                _ => {}
            }
        });
    }

    Ok(())
}

/// Simulate the OS-appropriate paste shortcut (Ctrl+V on Windows, Cmd+V on macOS).
pub fn send_ctrl_v() -> Result<()> {
    send_paste()
}

/// Simulate the OS-appropriate copy shortcut (Ctrl+C on Windows, Cmd+C on macOS).
pub fn send_ctrl_c() -> Result<()> {
    send_copy()
}

// ──────────────────────────────────────────────────────────────────────────────
// Windows implementation
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(target_os = "windows")]
fn send_paste() -> Result<()> {
    send_ctrl_combo_win(windows::Win32::UI::Input::KeyboardAndMouse::VK_V)
}

#[cfg(target_os = "windows")]
fn send_copy() -> Result<()> {
    send_ctrl_combo_win(windows::Win32::UI::Input::KeyboardAndMouse::VK_C)
}

#[cfg(target_os = "windows")]
fn send_ctrl_combo_win(vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY) -> Result<()> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
        VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
    };

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

#[cfg(target_os = "windows")]
fn key_input(
    vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY,
    key_up: bool,
) -> windows::Win32::UI::Input::KeyboardAndMouse::INPUT {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
    };
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

// ──────────────────────────────────────────────────────────────────────────────
// macOS implementation
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn send_paste() -> Result<()> {
    run_osascript("tell application \"System Events\" to keystroke \"v\" using command down")
}

#[cfg(target_os = "macos")]
fn send_copy() -> Result<()> {
    run_osascript("tell application \"System Events\" to keystroke \"c\" using command down")
}

#[cfg(target_os = "macos")]
fn run_osascript(script: &str) -> Result<()> {
    use std::process::Command;
    let status = Command::new("osascript")
        .args(["-e", script])
        .status()
        .context("failed to spawn osascript")?;
    if !status.success() {
        return Err(anyhow::anyhow!("osascript exited with status {status}"));
    }
    Ok(())
}

// ──────────────────────────────────────────────────────────────────────────────
// Other Unix / Linux fallback (compile-time stub — not functional)
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn send_paste() -> Result<()> {
    Err(anyhow::anyhow!(
        "inject_text not implemented for this platform"
    ))
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn send_copy() -> Result<()> {
    Err(anyhow::anyhow!(
        "send_copy not implemented for this platform"
    ))
}
