//! macOS focused-element reader via AXUIElement.
//!
//! Powers the correction-memory watcher (`correction.rs`). After Bulbul
//! pastes text, the watcher snapshots the focused field, polls it for
//! ~12 seconds while the user edits, and diffs the result to learn
//! corrections.
//!
//! Pattern adopted from a reference Swift implementation's AppContextService source (Mac
//! reference): get frontmost app via NSWorkspace, create an
//! AXUIElement for its PID, read `kAXFocusedUIElementAttribute` to get
//! the focused field, then read `kAXValueAttribute` (or
//! `kAXSelectedTextAttribute` as fallback) for the text content.
//!
//! `Reader::new()` returns None when AX permission hasn't been granted
//! yet — the correction watcher short-circuits and the user sees no
//! observable effect. Phase 6 adds an onboarding step that walks users
//! through granting the permission; until then they grant manually via
//! System Settings → Privacy & Security → Accessibility.

use std::ptr;

use accessibility_sys::{
    kAXErrorSuccess, kAXFocusedUIElementAttribute, kAXRoleAttribute,
    kAXSecureTextFieldSubrole, kAXSelectedTextAttribute, kAXValueAttribute, AXIsProcessTrusted,
    AXUIElementCopyAttributeValue, AXUIElementCreateApplication, AXUIElementGetTypeID,
    AXUIElementRef,
};
use core_foundation::base::{CFGetTypeID, CFRelease, CFTypeRef, TCFType};
use core_foundation::string::{CFString, CFStringRef};
use objc2_app_kit::NSWorkspace;

/// Owned wrapper around an `AXUIElementRef`. Drops the +1 CF retain on
/// drop. Not Send/Sync — `correction.rs` keeps these on the watcher
/// thread that creates them, so we don't need cross-thread guarantees.
pub struct ElementHandle {
    inner: AXUIElementRef,
}

impl Drop for ElementHandle {
    fn drop(&mut self) {
        if !self.inner.is_null() {
            // SAFETY: inner was obtained via a +1 retain (either
            // AXUIElementCreateApplication or AXUIElementCopyAttributeValue
            // with a result that passed AXUIElementGetTypeID check).
            unsafe { CFRelease(self.inner as *const _) };
        }
    }
}

pub struct Focused {
    pub element: ElementHandle,
    pub text: String,
    pub is_password: bool,
}

/// On Windows this initializes a COM apartment for the calling thread.
/// AX needs no apartment — just run the closure.
pub fn with_com<T>(f: impl FnOnce() -> T) -> T {
    f()
}

pub struct Reader {
    _private: (),
}

impl Reader {
    /// Returns None when AX permission hasn't been granted. The
    /// permission state is cached per-process by macOS itself, so we
    /// don't bother caching here — each Reader::new() is a single
    /// AXIsProcessTrusted call, which is a cheap lookup.
    pub fn new() -> Option<Self> {
        // SAFETY: AXIsProcessTrusted is a pure-query function, safe to
        // call from any thread.
        if unsafe { AXIsProcessTrusted() } {
            Some(Reader { _private: () })
        } else {
            tracing::debug!(
                "UIA Mac: AXIsProcessTrusted() == false; correction watcher disabled \
                 (grant Accessibility in System Settings to enable)"
            );
            None
        }
    }

    /// Snapshot the currently-focused element of the frontmost app.
    /// Returns None when no app is foregrounded, when the app doesn't
    /// expose accessibility (some Electron apps before they opt in,
    /// most games), or when the focused element has no readable text.
    pub fn read_focused(&self) -> Option<Focused> {
        // Frontmost app → PID. NSWorkspace.frontmostApplication can be
        // nil during Mission Control / system transitions.
        let workspace = unsafe { NSWorkspace::sharedWorkspace() };
        let app = unsafe { workspace.frontmostApplication() }?;
        let pid = unsafe { app.processIdentifier() };

        // SAFETY: AXUIElementCreateApplication is documented to return
        // a +1 reference (never null in practice, but we guard anyway).
        let app_element: AXUIElementRef = unsafe { AXUIElementCreateApplication(pid) };
        if app_element.is_null() {
            return None;
        }
        // Wrap immediately so the early-return paths CFRelease correctly.
        let _app_handle = ElementHandle { inner: app_element };

        let focused = read_attribute_as_element(app_element, kAXFocusedUIElementAttribute)?;

        let is_password = read_attribute_as_string(focused.inner, kAXRoleAttribute)
            .map(|role| role == kAXSecureTextFieldSubrole)
            .unwrap_or(false);

        // Text fields expose kAXValueAttribute; some rich editors only
        // expose kAXSelectedTextAttribute. Try both. Return None when
        // neither yields text — the watcher treats that as "not
        // observable, skip this dictation".
        let text = read_attribute_as_string(focused.inner, kAXValueAttribute)
            .or_else(|| read_attribute_as_string(focused.inner, kAXSelectedTextAttribute))?;

        Some(Focused {
            element: focused,
            text,
            is_password,
        })
    }

    /// Re-read the same element captured earlier, regardless of
    /// where focus has since moved. The watcher uses this to notice
    /// edits the user makes after Bulbul's paste.
    pub fn read_element_text(&self, element: &ElementHandle) -> Option<String> {
        read_attribute_as_string(element.inner, kAXValueAttribute)
            .or_else(|| read_attribute_as_string(element.inner, kAXSelectedTextAttribute))
    }
}

/// Read an AXUIElement-typed attribute. Returns an owned ElementHandle
/// (the caller drops it when done).
fn read_attribute_as_element(element: AXUIElementRef, attribute: &str) -> Option<ElementHandle> {
    let attr = CFString::new(attribute);
    let mut value: CFTypeRef = ptr::null();
    // SAFETY: AXUIElementCopyAttributeValue is documented to write into
    // `value` on success with a +1 retained CFType. We verify the type
    // before claiming ownership.
    let err = unsafe {
        AXUIElementCopyAttributeValue(element, attr.as_concrete_TypeRef(), &mut value)
    };
    if err != kAXErrorSuccess || value.is_null() {
        return None;
    }
    let type_id = unsafe { CFGetTypeID(value) };
    if type_id != unsafe { AXUIElementGetTypeID() } {
        // Wrong type — release and bail. (CFRelease on a non-AXUIElement
        // is still valid; CFGetTypeID just told us it's *some* CFType.)
        unsafe { CFRelease(value) };
        return None;
    }
    Some(ElementHandle {
        inner: value as AXUIElementRef,
    })
}

/// Read a CFString-typed attribute and convert to Rust String. Returns
/// None when the attribute doesn't exist, isn't a string, or is empty.
fn read_attribute_as_string(element: AXUIElementRef, attribute: &str) -> Option<String> {
    let attr = CFString::new(attribute);
    let mut value: CFTypeRef = ptr::null();
    let err = unsafe {
        AXUIElementCopyAttributeValue(element, attr.as_concrete_TypeRef(), &mut value)
    };
    if err != kAXErrorSuccess || value.is_null() {
        return None;
    }
    // SAFETY: wrap_under_create_rule consumes the +1 retain. If the
    // value isn't a CFString this panics — but AXUIElementCopyAttributeValue
    // with a string attribute is documented to always yield a CFString
    // when err == kAXErrorSuccess. Defensive check via CFGetTypeID
    // would only catch a framework bug; skip.
    let s = unsafe {
        let cf_string: CFString = CFString::wrap_under_create_rule(value as CFStringRef);
        cf_string.to_string()
    };
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}
