//! Minimal UI Automation reader used by the correction-memory watcher.
//!
//! After Bulbul pastes text into the focused field, we want to read that
//! field back a few seconds later to see whether the user edited what we
//! injected. UIA is the only cross-app way to read another process's text
//! content, but its coverage is uneven: classic Win32 edits and most native
//! controls expose `ValuePattern`; rich editors expose `TextPattern`; many
//! Electron/Chromium surfaces expose neither cleanly. Every read therefore
//! returns `Option` and the caller treats `None` as "this app isn't
//! observable" rather than an error.

use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationTextPattern,
    IUIAutomationValuePattern, UIA_TextPatternId, UIA_ValuePatternId,
};

/// A read of whatever element currently holds keyboard focus, plus a handle
/// to that element so the caller can re-read *the same field* later even
/// after focus has moved elsewhere (e.g. to a sibling pane in the same
/// window — VS Code's editor, terminal, and side panels all share one HWND).
pub struct Focused {
    pub element: IUIAutomationElement,
    pub text: String,
    pub is_password: bool,
}

/// Run `f` with a COM apartment initialized for the current thread, then
/// uninitialize. The watcher calls this once and does all of its polling
/// inside the closure so the apartment lives for the whole watch window.
pub fn with_com<T>(f: impl FnOnce() -> T) -> T {
    // SAFETY: paired CoUninitialize below. MTA so we don't need a message
    // pump; UIA is happy in a multithreaded apartment. RPC_E_CHANGED_MODE
    // (already initialized as STA on this thread) is harmless — we still ran
    // a CoInitialize that the matching CoUninitialize balances.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let out = f();
    unsafe {
        CoUninitialize();
    }
    out
}

/// Holds the UIA core object so the watcher can read repeatedly without
/// re-creating it on every poll. Must be constructed and used on a thread
/// that has called `with_com` (COM initialized).
pub struct Reader {
    automation: IUIAutomation,
}

impl Reader {
    pub fn new() -> Option<Self> {
        // SAFETY: COM is initialized by the surrounding `with_com`.
        let automation: IUIAutomation =
            unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }
                .map_err(|e| tracing::debug!("UIA: CoCreateInstance failed: {e:#}"))
                .ok()?;
        Some(Self { automation })
    }

    /// Snapshot the currently focused element: its text plus a handle to the
    /// element itself. Returns `None` when nothing is focused or the element
    /// exposes no readable text.
    pub fn read_focused(&self) -> Option<Focused> {
        // SAFETY: COM initialized; all pointers come from UIA and are checked.
        let element = unsafe { self.automation.GetFocusedElement() }
            .map_err(|e| tracing::debug!("UIA: GetFocusedElement failed: {e:#}"))
            .ok()?;
        let is_password =
            unsafe { element.CurrentIsPassword() }.map(|b| b.as_bool()).unwrap_or(false);
        let text = element_text(&element)?;
        Some(Focused {
            element,
            text,
            is_password,
        })
    }

    /// Re-read the text of a *specific* element captured earlier. Reads the
    /// live value of exactly that field regardless of where focus has since
    /// moved — this is what keeps us from mistaking a sibling pane's content
    /// for an edit of the field we actually injected into.
    pub fn read_element_text(&self, element: &IUIAutomationElement) -> Option<String> {
        element_text(element)
    }
}

/// Pull text out of an element. Tries `ValuePattern` (single/multi-line
/// edits, search boxes) first, then `TextPattern` (rich editors). Returns
/// `None` when neither pattern is supported or the field is empty.
fn element_text(element: &IUIAutomationElement) -> Option<String> {
    // SAFETY: COM initialized; all pointers come from UIA and are checked.
    unsafe {
        if let Ok(vp) =
            element.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
        {
            if let Ok(bstr) = vp.CurrentValue() {
                let text = bstr.to_string();
                if !text.is_empty() {
                    return Some(text);
                }
            }
        }
        if let Ok(tp) =
            element.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
        {
            if let Ok(range) = tp.DocumentRange() {
                if let Ok(bstr) = range.GetText(-1) {
                    let text = bstr.to_string();
                    if !text.is_empty() {
                        return Some(text);
                    }
                }
            }
        }
        None
    }
}
