//! macOS foreground-app detection via NSWorkspace.
//!
//! `foreground_app()` returns the frontmost app's bundle identifier
//! (e.g. `"com.apple.Safari"`) — the closest analog to the Windows
//! exe-basename used by the rest of the cleanup pipeline. `friendly_app_name`
//! / `style_category_for_app` in config.rs map bundle IDs to display
//! names + style categories.
//!
//! `foreground_hwnd()` returns the frontmost app's PID. The correction
//! watcher uses it only as an equality-check sentinel ("did the user
//! switch apps mid-correction"), so a PID is functionally equivalent
//! to Windows' HWND in that role.
//!
//! Both calls are cheap (NSWorkspace is a singleton cached by the OS),
//! so we don't bother caching ourselves. objc2-app-kit exposes
//! these NSWorkspace methods as safe — no unsafe block needed.

use super::AppInfo;
use objc2_app_kit::NSWorkspace;

pub fn foreground_app() -> Option<AppInfo> {
    let workspace = NSWorkspace::sharedWorkspace();
    let app = workspace.frontmostApplication()?;
    // Bundle id is the stable matching key; localizedName ("Safari",
    // "Antigravity") is the display name — so unmapped apps no longer surface
    // the raw `com.appname.xyz`. localizedName can be None for some processes;
    // callers fall back to the curated table / raw id in that case.
    let bundle_id = app.bundleIdentifier()?;
    let display = app.localizedName().map(|n| n.to_string());
    Some(AppInfo {
        id: bundle_id.to_string(),
        display,
    })
}

pub fn foreground_hwnd() -> isize {
    let workspace = NSWorkspace::sharedWorkspace();
    let Some(app) = workspace.frontmostApplication() else {
        return 0;
    };
    // processIdentifier returns pid_t (i32). Widen to isize for cross-platform
    // signature parity with Windows (which returns the HWND cast to isize).
    app.processIdentifier() as isize
}
