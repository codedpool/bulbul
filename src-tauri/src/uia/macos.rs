//! macOS focused-element reader — stub.
//!
//! Phase 5 swaps these for AXUIElementCreateApplication(pid) +
//! kAXFocusedUIElementAttribute + kAXValueAttribute. The `element`
//! handle becomes an AXUIElementRef; reads stay best-effort and return
//! None when the foreground app doesn't expose AX (some Electron apps,
//! most games). See `macos-port-plan.md` Phase 5.
//!
//! Today: `Reader::new()` returns None, so the correction watcher in
//! `correction.rs` short-circuits before any of the stub structs are
//! constructed. The structs exist only so the call sites compile.

/// Stand-in for `IUIAutomationElement`. Real Phase 5 implementation will
/// hold an `AXUIElementRef` (AX-side handle to the captured field, used by
/// the correction watcher to re-read the same field after focus moves to
/// a sibling pane).
pub struct ElementHandle;

/// Mirror of the Windows-side struct. `element` is intentionally typed as
/// the stub `ElementHandle` for now — once `Reader::new()` returns Some on
/// macOS, real AXUIElementRef handles flow through here.
pub struct Focused {
    pub element: ElementHandle,
    pub text: String,
    pub is_password: bool,
}

/// On Windows this initializes a COM apartment for the calling thread.
/// On macOS AX needs no apartment, so we just run the closure straight.
pub fn with_com<T>(f: impl FnOnce() -> T) -> T {
    f()
}

pub struct Reader;

impl Reader {
    pub fn new() -> Option<Self> {
        // Phase 5 will check AXIsProcessTrusted() and return Some only
        // when accessibility permission has been granted. Today the
        // entire AX pipeline is a no-op so the correction watcher
        // short-circuits here.
        None
    }

    pub fn read_focused(&self) -> Option<Focused> {
        None
    }

    pub fn read_element_text(&self, _element: &ElementHandle) -> Option<String> {
        None
    }
}
