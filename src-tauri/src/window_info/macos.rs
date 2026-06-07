//! macOS foreground-app detection via NSWorkspace.
//!
//! `foreground_app()` returns the frontmost app's bundle identifier
//! (e.g. `"com.apple.Safari"`) — the closest analog to the Windows
//! exe-basename used by the rest of the cleanup pipeline. `friendly_app_name`
//! / `style_category_for_app` in config.rs map bundle IDs to display
//! names + style categories (Phase 7 extends those tables).
//!
//! `foreground_hwnd()` returns the frontmost app's PID. The correction
//! watcher uses it only as an equality-check sentinel ("did the user
//! switch apps mid-correction"), so a PID is functionally equivalent
//! to Windows' HWND in that role.
//!
//! Both calls are cheap (NSWorkspace is a singleton cached by the OS),
//! so we don't bother caching ourselves.

use objc2_app_kit::NSWorkspace;

pub fn foreground_app() -> Option<String> {
    // SAFETY: NSWorkspace.sharedWorkspace is documented as thread-safe and
    // returns a valid singleton on every call. frontmostApplication can
    // return nil (no app foregrounded — e.g. during Mission Control); we
    // surface that as None.
    let workspace = unsafe { NSWorkspace::sharedWorkspace() };
    let app = unsafe { workspace.frontmostApplication() }?;
    let bundle_id = unsafe { app.bundleIdentifier() }?;
    Some(bundle_id.to_string())
}

pub fn foreground_hwnd() -> isize {
    // SAFETY: same justification as foreground_app.
    let workspace = unsafe { NSWorkspace::sharedWorkspace() };
    let Some(app) = (unsafe { workspace.frontmostApplication() }) else {
        return 0;
    };
    // processIdentifier returns pid_t (i32). Widen to isize for cross-platform
    // signature parity with Windows (which returns the HWND cast to isize).
    unsafe { app.processIdentifier() as isize }
}
