// ──────────────────────────────────────────────────────────────────────────────
// Windows implementation
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(target_os = "windows")]
use windows::Win32::Foundation::CloseHandle;
#[cfg(target_os = "windows")]
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
};
#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

/// Return the executable name (without path) of the foreground window's
/// process, e.g. "Code.exe", "slack.exe". Returns None on any failure.
#[cfg(target_os = "windows")]
pub fn foreground_app() -> Option<String> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_invalid() {
            return None;
        }
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            return None;
        }
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;

        let mut buf = [0u16; 1024];
        let mut size = buf.len() as u32;
        let res = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_FORMAT(0),
            windows::core::PWSTR(buf.as_mut_ptr()),
            &mut size,
        );
        let _ = CloseHandle(handle);
        res.ok()?;

        let full = String::from_utf16_lossy(&buf[..size as usize]);
        full.rsplit(['\\', '/']).next().map(|s| s.to_string())
    }
}

/// Raw handle of the current foreground window, as an `isize` for cheap
/// equality checks. Returns 0 when there is no foreground window.
#[cfg(target_os = "windows")]
pub fn foreground_hwnd() -> isize {
    unsafe { GetForegroundWindow().0 as isize }
}

// ──────────────────────────────────────────────────────────────────────────────
// macOS implementation
// ──────────────────────────────────────────────────────────────────────────────

/// Return the name of the frontmost application (e.g. "Code", "Slack").
/// Uses `osascript` to query the System Events process list.
#[cfg(target_os = "macos")]
pub fn foreground_app() -> Option<String> {
    use std::process::Command;
    let output = Command::new("osascript")
        .args([
            "-e",
            "tell application \"System Events\" to get name of first process where it is frontmost",
        ])
        .output()
        .ok()?;
    if output.status.success() {
        let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !name.is_empty() {
            return Some(name);
        }
    }
    None
}

/// Not meaningful on macOS — returns 0 (used only for change-detection equality checks).
#[cfg(target_os = "macos")]
pub fn foreground_hwnd() -> isize {
    0
}

// ──────────────────────────────────────────────────────────────────────────────
// Fallback (other Unix / Linux)
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn foreground_app() -> Option<String> {
    None
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn foreground_hwnd() -> isize {
    0
}
