//! Linux text injection — stub.
//!
//! Phase 3 replaces these with:
//! - X11: arboard clipboard + XTestFakeKeyEvent for Ctrl+V, with
//!   modifier-release polling via `xcb_query_keymap` (same idea as
//!   the Windows GetAsyncKeyState path).
//! - Wayland: ashpd / reis to negotiate a libei keyboard device and
//!   post Ctrl+V through that, falling back to the RemoteDesktop
//!   portal on older compositors.
//!
//! Today these return errors so the orchestrator surfaces a clear
//! "not implemented" message rather than crashing.

use anyhow::Result;

pub fn inject_text(_text: &str) -> Result<()> {
    Err(anyhow::anyhow!(
        "text injection not yet implemented on Linux (Phase 3)"
    ))
}

pub fn send_ctrl_v() -> Result<()> {
    Err(anyhow::anyhow!(
        "paste keystroke not yet implemented on Linux (Phase 3)"
    ))
}

pub fn send_ctrl_c() -> Result<()> {
    Err(anyhow::anyhow!(
        "copy keystroke not yet implemented on Linux (Phase 3)"
    ))
}
