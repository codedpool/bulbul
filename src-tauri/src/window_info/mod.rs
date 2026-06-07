//! Foreground-app detection. The cleanup pipeline + correction watcher
//! both need to know which app the user is dictating into.
//!
//! Per-platform impl is in `windows.rs` / `macos.rs`. Callers do
//! `crate::window_info::foreground_app()` without caring.

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::*;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;
