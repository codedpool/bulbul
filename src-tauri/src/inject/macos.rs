//! macOS text injection — stub.
//!
//! Real implementation lands in Phase 3 of the macOS port (clipboard +
//! CGEvent for Cmd+V at `cghidEventTap`, modifier-release before paste,
//! prior-clipboard snapshot/restore). See `macos-port-plan.md` Phase 3.
//!
//! Until then these functions return errors so the orchestrator surfaces
//! a clear failure rather than crashing.

use anyhow::Result;

pub fn inject_text(_text: &str) -> Result<()> {
    Err(anyhow::anyhow!(
        "text injection not yet implemented on macOS (Phase 3)"
    ))
}

pub fn send_ctrl_v() -> Result<()> {
    Err(anyhow::anyhow!(
        "paste keystroke not yet implemented on macOS (Phase 3)"
    ))
}

pub fn send_ctrl_c() -> Result<()> {
    Err(anyhow::anyhow!(
        "copy keystroke not yet implemented on macOS (Phase 3)"
    ))
}
