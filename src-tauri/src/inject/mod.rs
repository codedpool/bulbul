//! Text injection into the focused app: writes to clipboard, fires the
//! OS-level "paste" combo (Ctrl+V on Windows, Cmd+V on macOS), restores
//! the prior clipboard contents after a short delay.
//!
//! Per-platform implementation lives in `windows.rs` / `macos.rs`. This
//! module just re-exports the chosen one so callers say
//! `crate::inject::inject_text(...)` without caring about the OS.

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
#[cfg(target_os = "linux")]
pub mod linux_portal_paste;
