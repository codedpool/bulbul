// ──────────────────────────────────────────────────────────────────────────────
// Windows implementation — full COM / UI Automation
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(target_os = "windows")]
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
#[cfg(target_os = "windows")]
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationTextPattern,
    IUIAutomationValuePattern, UIA_TextPatternId, UIA_ValuePatternId,
};

/// A read of whatever element currently holds keyboard focus.
#[cfg(target_os = "windows")]
pub struct Focused {
    pub element: IUIAutomationElement,
    pub text: String,
    pub is_password: bool,
}

/// Run `f` with a COM apartment initialized for the current thread.
#[cfg(target_os = "windows")]
pub fn with_com<T>(f: impl FnOnce() -> T) -> T {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let out = f();
    unsafe {
        CoUninitialize();
    }
    out
}

/// Holds the UIA core object so the watcher can read repeatedly.
#[cfg(target_os = "windows")]
pub struct Reader {
    automation: IUIAutomation,
}

#[cfg(target_os = "windows")]
impl Reader {
    pub fn new() -> Option<Self> {
        let automation: IUIAutomation =
            unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }
                .map_err(|e| tracing::debug!("UIA: CoCreateInstance failed: {e:#}"))
                .ok()?;
        Some(Self { automation })
    }

    pub fn read_focused(&self) -> Option<Focused> {
        let element = unsafe { self.automation.GetFocusedElement() }
            .map_err(|e| tracing::debug!("UIA: GetFocusedElement failed: {e:#}"))
            .ok()?;
        let is_password = unsafe { element.CurrentIsPassword() }
            .map(|b| b.as_bool())
            .unwrap_or(false);
        let text = element_text(&element)?;
        Some(Focused {
            element,
            text,
            is_password,
        })
    }

    pub fn read_element_text(&self, element: &IUIAutomationElement) -> Option<String> {
        element_text(element)
    }
}

#[cfg(target_os = "windows")]
fn element_text(element: &IUIAutomationElement) -> Option<String> {
    unsafe {
        if let Ok(vp) = element.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
        {
            if let Ok(bstr) = vp.CurrentValue() {
                let text = bstr.to_string();
                if !text.is_empty() {
                    return Some(text);
                }
            }
        }
        if let Ok(tp) = element.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId) {
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

// ──────────────────────────────────────────────────────────────────────────────
// Non-Windows stub — correction memory is not supported outside Windows.
// Reader::new() always returns None so the watcher exits immediately.
// ──────────────────────────────────────────────────────────────────────────────

/// Placeholder element type for non-Windows platforms.
#[cfg(not(target_os = "windows"))]
pub struct Focused {
    /// Opaque placeholder — not meaningful outside Windows.
    pub element: (),
    pub text: String,
    pub is_password: bool,
}

/// No-op COM wrapper — just calls the callback directly.
#[cfg(not(target_os = "windows"))]
pub fn with_com<T>(f: impl FnOnce() -> T) -> T {
    f()
}

/// Stub reader — always returns `None` from `new()` so callers exit early.
#[cfg(not(target_os = "windows"))]
pub struct Reader;

#[cfg(not(target_os = "windows"))]
impl Reader {
    pub fn new() -> Option<Self> {
        None
    }

    pub fn read_focused(&self) -> Option<Focused> {
        None
    }

    pub fn read_element_text(&self, _element: &()) -> Option<String> {
        None
    }
}
