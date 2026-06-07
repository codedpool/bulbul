//! Linux foreground-app detection — stub.
//!
//! Phase 2 replaces these with:
//! - X11: `x11rb` XGetInputFocus → walk to top-level window →
//!   XGetClassHint, return the WM_CLASS class field (e.g. `"firefox"`,
//!   `"code"`, `"org.gnome.Terminal"`).
//! - Wayland: no generic API exists. Returns None by design; the
//!   cleanup pipeline degrades gracefully (no venue hint, neutral
//!   style). Documented in README + onboarding.

pub fn foreground_app() -> Option<String> {
    None
}

/// On X11: the X Window ID of the focused window, cast to isize, used
/// by the correction watcher as a "did user switch apps" signal.
/// On Wayland: 0 (unknown).
pub fn foreground_hwnd() -> isize {
    0
}
