//! Foreground-app detection. The cleanup pipeline + correction watcher
//! both need to know which app the user is dictating into.
//!
//! Per-platform impl is in `windows.rs` / `macos.rs`. Callers do
//! `crate::window_info::foreground_app()` without caring.

/// A detected foreground app.
///
/// `id` is the stable identifier the rest of the pipeline matches on —
/// Windows exe stem (`Code.exe`), macOS bundle id (`com.apple.Safari`),
/// Linux WM_CLASS (`firefox`). It keys corrections, per-app Style, and the
/// curated name table, so it must stay stable across locales/sessions.
///
/// `display` is a human-readable name when the OS can give us one directly
/// (macOS `localizedName`). It's `None` on platforms/paths that only expose
/// the id; callers then fall back to `config::friendly_app_name(id)`.
pub struct AppInfo {
    pub id: String,
    pub display: Option<String>,
}

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::*;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::*;
