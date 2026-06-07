//! macOS foreground-app detection — stub.
//!
//! Phase 2 swaps these for NSWorkspace.sharedWorkspace.frontmostApplication
//! (bundle ID + localized name) and uses the running app's PID as the
//! `foreground_hwnd` analog. See `macos-port-plan.md` Phase 2.

pub fn foreground_app() -> Option<String> {
    None
}

/// `foreground_hwnd` semantics on Mac: a stable id for "the currently
/// focused window" used only for equality checks (the correction watcher
/// uses it to notice the user clicked away). PID of the frontmost app is
/// the planned replacement; returning 0 here is "unknown / no foreground"
/// so equality checks always look like "user switched" — safe but means
/// the correction watcher exits early on Mac until Phase 2 lands.
pub fn foreground_hwnd() -> isize {
    0
}
