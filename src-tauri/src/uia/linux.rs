//! Linux focused-element reader — stub.
//!
//! Phase 5 replaces these with AT-SPI (`atspi-rs` crate). AT-SPI is the
//! cross-desktop accessibility standard, exposed over D-Bus and
//! identical on X11 and Wayland. Coverage is good on GNOME, spottier
//! on KDE — same caveats as UIA on Windows.
//!
//! Today `Reader::new()` returns None so the correction watcher in
//! `correction.rs` short-circuits before any stub struct is built.

/// Stand-in for the X11/AT-SPI element handle that Phase 5 will define.
pub struct ElementHandle;

/// Mirrors the Windows-side `Focused` struct so `correction.rs` compiles
/// unchanged across all three platforms.
pub struct Focused {
    pub element: ElementHandle,
    pub text: String,
    pub is_password: bool,
}

/// On Windows this initializes a COM apartment for the calling thread.
/// AT-SPI is D-Bus based — no apartment needed; just run the closure.
pub fn with_com<T>(f: impl FnOnce() -> T) -> T {
    f()
}

pub struct Reader;

impl Reader {
    pub fn new() -> Option<Self> {
        // Phase 5 will connect to the AT-SPI registry (D-Bus) and check
        // whether accessibility is enabled. Today the pipeline is a no-op.
        None
    }

    pub fn read_focused(&self) -> Option<Focused> {
        None
    }

    pub fn read_element_text(&self, _element: &ElementHandle) -> Option<String> {
        None
    }
}
