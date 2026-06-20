//! Bulbul library entry point.
//!
//! The crate compiles for desktop (Windows/macOS/Linux) AND for mobile
//! (Android — iOS later). The two paths share very little beyond the
//! Tauri webview shell: desktop has global hotkeys, tray icons, audio
//! capture via cpal, modifier-chord polling, AccessibilityNodeInfo-
//! equivalent injection on each OS, etc.; mobile gets a foreground
//! service + accessibility-service-driven overlay bubble (planned),
//! with audio captured through Kotlin's AudioRecord and bridged in via
//! JNI.
//!
//! Rather than thread `#[cfg(desktop)]` through 2400 lines of run()
//! setup, we split this file into two siblings textually included by
//! cfg, so desktop builds see the existing implementation unchanged
//! and mobile builds compile a much smaller surface that grows as we
//! port features over.
//!
//!  - [`./desktop.rs`] — every desktop-only module, command, plugin,
//!    window, and the orchestrator. The file kept its original lib.rs
//!    layout so blame/history stays continuous.
//!  - [`./mobile.rs`] — the minimal Tauri builder + JNI-friendly
//!    `pub fn run()` decorated with `mobile_entry_point`. Stubs for
//!    invoke commands the frontend calls go here as features land.

#[cfg(desktop)]
include!("./desktop.rs");

#[cfg(mobile)]
include!("./mobile.rs");
