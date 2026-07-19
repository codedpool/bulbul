use super::AppInfo;
use std::ffi::c_void;
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

/// Return the foreground window's process as an `AppInfo`. `id` is the
/// executable name (e.g. "Code.exe", "WhatsApp.Root.exe") — the stable
/// matching key. `display` is the exe's FileVersionInfo description (the
/// friendly name Task Manager shows, e.g. "WhatsApp"), so unmapped exes still
/// render a real name. Returns None on failure.
pub fn foreground_app() -> Option<AppInfo> {
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
            PWSTR(buf.as_mut_ptr()),
            &mut size,
        );
        let _ = CloseHandle(handle);
        res.ok()?;

        let full = String::from_utf16_lossy(&buf[..size as usize]);
        let name = full.rsplit(['\\', '/']).next()?.to_string();
        let display = version_display_name(&full);
        Some(AppInfo { id: name, display })
    }
}

/// NUL-terminated UTF-16 for the Win32 wide-string APIs.
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Pull a human-readable app name out of an executable's version resource:
/// FileDescription (what Task Manager shows — "WhatsApp", "Google Chrome"),
/// falling back to ProductName. Returns None when the exe has no version
/// resource or the fields are empty (many apps ship none — caller then falls
/// back to the exe stem / curated table).
fn version_display_name(path: &str) -> Option<String> {
    let wpath = wide(path);
    unsafe {
        let size = GetFileVersionInfoSizeW(PCWSTR(wpath.as_ptr()), None);
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        GetFileVersionInfoW(PCWSTR(wpath.as_ptr()), 0, size, data.as_mut_ptr() as *mut c_void)
            .ok()?;

        // Language + codepage from the translation table drive the string
        // sub-block path. Fall back to US-English/Unicode if it's missing.
        let (lang, codepage) = {
            let mut ptr: *mut c_void = std::ptr::null_mut();
            let mut len: u32 = 0;
            let key = wide("\\VarFileInfo\\Translation");
            if VerQueryValueW(
                data.as_ptr() as *const c_void,
                PCWSTR(key.as_ptr()),
                &mut ptr,
                &mut len,
            )
            .as_bool()
                && !ptr.is_null()
                && len >= 4
            {
                (*(ptr as *const u16), *((ptr as *const u16).add(1)))
            } else {
                (0x0409, 0x04b0)
            }
        };

        for field in ["FileDescription", "ProductName"] {
            let sub = format!("\\StringFileInfo\\{:04x}{:04x}\\{}", lang, codepage, field);
            let wsub = wide(&sub);
            let mut ptr: *mut c_void = std::ptr::null_mut();
            let mut len: u32 = 0;
            if VerQueryValueW(
                data.as_ptr() as *const c_void,
                PCWSTR(wsub.as_ptr()),
                &mut ptr,
                &mut len,
            )
            .as_bool()
                && !ptr.is_null()
                && len > 0
            {
                let slice = std::slice::from_raw_parts(ptr as *const u16, len as usize);
                let s = String::from_utf16_lossy(slice);
                let s = s.trim_end_matches('\0').trim();
                if !s.is_empty() {
                    return Some(s.to_string());
                }
            }
        }
        None
    }
}

/// Raw handle of the current foreground window, as an `isize` for cheap
/// equality checks. The correction watcher uses this to notice when the user
/// clicks away from the field they just dictated into (= they're done
/// editing). Returns 0 when there is no foreground window.
pub fn foreground_hwnd() -> isize {
    unsafe { GetForegroundWindow().0 as isize }
}

#[cfg(test)]
mod tests {
    use super::version_display_name;

    #[test]
    fn reads_a_version_name_from_a_system_exe() {
        // A stable system exe that ships a version resource. The exact
        // FileDescription is locale-dependent, so just assert the FFI path
        // extracts *something* non-empty end to end.
        let name = version_display_name("C:\\Windows\\System32\\cmd.exe");
        assert!(name.as_deref().is_some_and(|s| !s.is_empty()), "got {name:?}");
    }

    #[test]
    fn returns_none_for_a_nonexistent_path() {
        assert_eq!(version_display_name("Z:\\nope\\does-not-exist.exe"), None);
    }
}
