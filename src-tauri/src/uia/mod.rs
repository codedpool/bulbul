//! Focused-element text reader used by the correction-memory watcher.
//!
//! Named `uia` (UI Automation) for historical Windows reasons; on macOS
//! the implementation will use AXUIElement instead. The public surface is
//! kept stable so `correction.rs` doesn't need to care.
//!
//! Per-platform impl is in `windows.rs` / `macos.rs`. See `macos-port-plan.md`
//! Phase 5 for the Mac implementation.

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::*;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;
