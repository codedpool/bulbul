//! macOS text injection: NSPasteboard clipboard + Cmd+V via CGEvent.
//!
//! 1. Snapshot the user's prior clipboard contents (NSPasteboard read).
//! 2. Write the transcript to the clipboard with TRANSIENT pasteboard
//!    types declared, so well-behaved clipboard managers (Raycast,
//!    Maccy, Paste, Clipy, Flycut) skip recording the dictation into
//!    their history. The text still pastes normally via Cmd+V — only
//!    clipboard managers see the marker types.
//! 3. **Wait for any held modifiers to be released**, up to 600ms.
//!    Otherwise the paste fires while ⌘/⌥/⌃/⇧ are still down from the
//!    dictation hotkey and Cmd+V becomes something else.
//! 4. Wait another 30ms (lets the clipboard settle and gives the OS a
//!    moment between modifier-release and our synthetic event).
//! 5. Post two CGEvents (V keydown + V keyup) with `.maskCommand`,
//!    targeting `.cgSessionEventTap`. The V keycode is resolved via
//!    TIS/UCKeyTranslate so the paste still works under Dvorak,
//!    AZERTY, Workman, etc.
//! 6. After 1s, restore the prior clipboard IF the clipboard still
//!    holds exactly what we wrote (so the user can copy something
//!    new mid-paste and we won't clobber it).
//!
//! See:
//!   <https://github.com/nicke5012/TransientPasteboardType>
//!     — the convention clipboard managers honor for skip-record hints.

use anyhow::{anyhow, Context, Result};
use core_foundation::base::{CFRelease, CFTypeRef};
use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2_foundation::{NSArray, NSString};
use std::ptr;
use std::thread;
use std::time::Duration;

// ----- Modifier keycodes for the pre-paste release wait. ------------------
// These are virtual-keycode constants (kVK_*) which are layout-INDEPENDENT
// on macOS — the Cmd key is always 55/54 regardless of QWERTY/Dvorak/etc.
const MODIFIER_KEYCODES: &[u16] = &[
    55, // Command (Left)
    54, // Command (Right)
    56, // Shift (Left)
    60, // Shift (Right)
    58, // Option (Left)
    61, // Option (Right)
    59, // Control (Left)
    62, // Control (Right)
];

// ----- Tuning constants. --------------------------------------------------
const MOD_POLL_INTERVAL: Duration = Duration::from_millis(25);
const MOD_POLL_MAX_ATTEMPTS: u32 = 24; // = 600ms ceiling
const POST_RELEASE_DELAY: Duration = Duration::from_millis(30);
const CLIPBOARD_SETTLE: Duration = Duration::from_millis(40);
const CLIPBOARD_RESTORE_DELAY: Duration = Duration::from_millis(1000);

// US-QWERTY fallback keycodes for V and C, used when TIS lookup fails
// (e.g. non-Roman keyboard layouts where 'v' doesn't appear in the
// translated layer). On US QWERTY these are correct; on Dvorak/AZERTY/
// etc. the TIS lookup below resolves the real position.
const FALLBACK_KEYCODE_V: u16 = 9;
const FALLBACK_KEYCODE_C: u16 = 8;

// ----- FFI: CGEventSourceKeyState ------------------------------------------
// CGEventSourceKeyState isn't bound by the Rust core-graphics crate. The
// C signature is documented as a pure-query function, safe from any
// thread. C99 `bool` is ABI-compatible with Rust's `bool`.
extern "C" {
    fn CGEventSourceKeyState(state_id: CGEventSourceStateID, key: u16) -> bool;
}

// ----- FFI: TIS + UCKeyTranslate -------------------------------------------
// Carbon is the umbrella framework that exposes the modern TIS APIs.
// They're not marked deprecated in current SDKs (the rest of Carbon is).
#[link(name = "Carbon", kind = "framework")]
extern "C" {
    fn TISCopyCurrentKeyboardInputSource() -> CFTypeRef;
    fn TISGetInputSourceProperty(source: CFTypeRef, property_key: CFTypeRef) -> CFTypeRef;
    fn UCKeyTranslate(
        keyboard_layout_ptr: *const u8,
        virtual_key_code: u16,
        key_action: u16,
        modifier_key_state: u32,
        keyboard_type: u32,
        key_translate_options: u32,
        dead_key_state: *mut u32,
        max_string_length: usize,
        actual_string_length: *mut usize,
        unicode_string: *mut u16,
    ) -> i32;
    fn LMGetKbdType() -> u8;
    fn CFDataGetBytePtr(data: CFTypeRef) -> *const u8;
    static kTISPropertyUnicodeKeyLayoutData: CFTypeRef;
}

const UC_KEY_ACTION_DISPLAY: u16 = 3;
const UC_KEY_TRANSLATE_NO_DEAD_KEYS_BIT: u32 = 1;
const NO_ERR: i32 = 0;

/// Walk the current keyboard layout's keycode table to find which
/// position produces the given character when pressed without
/// modifiers. Returns None when the character isn't reachable on the
/// current layout (e.g. a Roman letter on a Cyrillic-only layout).
fn keycode_for_character(target: char) -> Option<u16> {
    // SAFETY: TISCopyCurrentKeyboardInputSource returns a +1 retained
    // CFType ref; we CFRelease it before return. TISGetInputSourceProperty
    // returns an unretained ref (no release needed). UCKeyTranslate is
    // a pure function over the layout data.
    unsafe {
        let source = TISCopyCurrentKeyboardInputSource();
        if source.is_null() {
            return None;
        }
        let layout_data_ref =
            TISGetInputSourceProperty(source, kTISPropertyUnicodeKeyLayoutData);
        if layout_data_ref.is_null() {
            CFRelease(source);
            return None;
        }
        let layout_ptr = CFDataGetBytePtr(layout_data_ref);
        if layout_ptr.is_null() {
            CFRelease(source);
            return None;
        }
        let kbd_type = LMGetKbdType() as u32;
        let mut result: Option<u16> = None;
        for keycode in 0u16..128 {
            let mut chars = [0u16; 4];
            let mut char_count: usize = 0;
            let mut dead_key_state: u32 = 0;
            let status = UCKeyTranslate(
                layout_ptr,
                keycode,
                UC_KEY_ACTION_DISPLAY,
                0, // modifier_key_state — unmodified base layer
                kbd_type,
                UC_KEY_TRANSLATE_NO_DEAD_KEYS_BIT,
                &mut dead_key_state,
                4,
                &mut char_count,
                chars.as_mut_ptr(),
            );
            if status == NO_ERR && char_count > 0 {
                if let Some(c) = char::from_u32(chars[0] as u32) {
                    if c == target {
                        result = Some(keycode);
                        break;
                    }
                }
            }
        }
        CFRelease(source);
        result
    }
}

fn paste_keycode() -> u16 {
    keycode_for_character('v').unwrap_or(FALLBACK_KEYCODE_V)
}

fn copy_keycode() -> u16 {
    keycode_for_character('c').unwrap_or(FALLBACK_KEYCODE_C)
}

// ----- Public API ----------------------------------------------------------

pub fn inject_text(text: &str) -> Result<()> {
    if text.is_empty() {
        return Ok(());
    }

    // Snapshot existing clipboard before we overwrite. None means the
    // pasteboard was empty (or held non-string content) — we just skip
    // the restore in that case.
    let previous = read_pasteboard_text().ok();

    let payload = text.to_string();
    write_pasteboard_text_transient(&payload).context("writing transcript to pasteboard")?;

    thread::sleep(CLIPBOARD_SETTLE);

    wait_for_modifiers_released();

    post_command_combo(paste_keycode()).context("posting Cmd+V")?;

    // Background restore. Only restore if the clipboard still holds
    // *our* paste — that way a user-initiated mid-paste copy isn't
    // clobbered. Uses the plain-string write path (no transient
    // markers) since we're restoring the user's prior content, not a
    // dictation result.
    if let Some(prev) = previous {
        let payload_for_restore = payload.clone();
        thread::spawn(move || {
            thread::sleep(CLIPBOARD_RESTORE_DELAY);
            let current = read_pasteboard_text().ok();
            if current.as_deref() == Some(payload_for_restore.as_str()) {
                let _ = write_pasteboard_text_plain(&prev);
            }
        });
    }

    Ok(())
}

/// Synthesize Cmd+V at the Session-level event tap. Used by the
/// Transform pipeline after writing transformed text to the clipboard.
pub fn send_ctrl_v() -> Result<()> {
    post_command_combo(paste_keycode())
}

/// Synthesize Cmd+C at the Session-level event tap. Used by the
/// Transform pipeline to copy the user's selection before processing.
pub fn send_ctrl_c() -> Result<()> {
    post_command_combo(copy_keycode())
}

// ----- CGEvent post --------------------------------------------------------

fn post_command_combo(keycode: u16) -> Result<()> {
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow!("CGEventSource::new failed"))?;

    let key_down = CGEvent::new_keyboard_event(source.clone(), keycode, true)
        .map_err(|_| anyhow!("CGEvent::new_keyboard_event(keyDown) failed"))?;
    key_down.set_flags(CGEventFlags::CGEventFlagCommand);
    key_down.post(CGEventTapLocation::Session);

    let key_up = CGEvent::new_keyboard_event(source, keycode, false)
        .map_err(|_| anyhow!("CGEvent::new_keyboard_event(keyUp) failed"))?;
    key_up.set_flags(CGEventFlags::CGEventFlagCommand);
    key_up.post(CGEventTapLocation::Session);

    Ok(())
}

// ----- NSPasteboard helpers ------------------------------------------------
//
// Bulbul uses NSPasteboard directly (rather than arboard) on Mac so the
// dictation write can declare transient/concealed/auto-generated
// pasteboard types. Clipboard managers honor these UTIs as a "skip
// recording this entry" hint, so the user's history doesn't fill up
// with every dictation.

/// `public.utf8-plain-text` is the underlying UTI for
/// `NSPasteboardTypeString`. Hardcoded so we don't need a Carbon/AppKit
/// symbol lookup at runtime.
const PB_TYPE_STRING: &str = "public.utf8-plain-text";
const PB_TYPE_TRANSIENT: &str = "org.nspasteboard.TransientType";
const PB_TYPE_CONCEALED: &str = "org.nspasteboard.ConcealedType";
const PB_TYPE_AUTOGEN: &str = "org.nspasteboard.AutoGeneratedType";
const PB_TYPE_LEGACY_TRANSIENT: &str = "de.petermaurer.TransientPasteboardType";

/// Grab the singleton `[NSPasteboard generalPasteboard]`.
fn general_pasteboard() -> Result<Retained<AnyObject>> {
    // SAFETY: NSPasteboard.generalPasteboard is a class method
    // returning an unretained autoreleased reference. msg_send! with a
    // Retained<AnyObject> return type promotes it to +1 retain via
    // objc2's automatic memory-management bridge.
    unsafe {
        let cls = AnyClass::get(c"NSPasteboard")
            .ok_or_else(|| anyhow!("NSPasteboard class not found (AppKit linkage?)"))?;
        let pb: Retained<AnyObject> = msg_send![cls, generalPasteboard];
        Ok(pb)
    }
}

/// Write the dictation result with all four transient-style UTIs
/// declared. The text reaches the focused app via the standard
/// `public.utf8-plain-text` payload; the marker types tell clipboard
/// managers to skip recording. Each marker type is given an empty
/// string payload because some clipboard managers check for data
/// presence rather than just declared type.
fn write_pasteboard_text_transient(text: &str) -> Result<()> {
    let pb = general_pasteboard()?;
    unsafe {
        let string_type = NSString::from_str(PB_TYPE_STRING);
        let transient = NSString::from_str(PB_TYPE_TRANSIENT);
        let concealed = NSString::from_str(PB_TYPE_CONCEALED);
        let autogen = NSString::from_str(PB_TYPE_AUTOGEN);
        let legacy = NSString::from_str(PB_TYPE_LEGACY_TRANSIENT);
        let types: Retained<NSArray<NSString>> = NSArray::from_slice(&[
            &*string_type,
            &*transient,
            &*concealed,
            &*autogen,
            &*legacy,
        ]);

        let _: i64 = msg_send![&*pb, declareTypes: &*types, owner: ptr::null_mut::<AnyObject>()];

        let payload = NSString::from_str(text);
        let _: bool = msg_send![&*pb, setString: &*payload, forType: &*string_type];

        let empty = NSString::from_str("");
        let _: bool = msg_send![&*pb, setString: &*empty, forType: &*transient];
        let _: bool = msg_send![&*pb, setString: &*empty, forType: &*concealed];
        let _: bool = msg_send![&*pb, setString: &*empty, forType: &*autogen];
        let _: bool = msg_send![&*pb, setString: &*empty, forType: &*legacy];
    }
    Ok(())
}

/// Write a plain `public.utf8-plain-text` payload — used to restore
/// the user's prior clipboard content. No transient markers so
/// clipboard managers happily record it again as the user's own
/// content.
fn write_pasteboard_text_plain(text: &str) -> Result<()> {
    let pb = general_pasteboard()?;
    unsafe {
        let string_type = NSString::from_str(PB_TYPE_STRING);
        let types: Retained<NSArray<NSString>> = NSArray::from_slice(&[&*string_type]);
        let _: i64 = msg_send![&*pb, declareTypes: &*types, owner: ptr::null_mut::<AnyObject>()];
        let payload = NSString::from_str(text);
        let _: bool = msg_send![&*pb, setString: &*payload, forType: &*string_type];
    }
    Ok(())
}

/// Read the current `public.utf8-plain-text` payload. Returns Err when
/// the pasteboard is empty or holds non-string content (image,
/// file URL, etc.) — callers treat this as "skip restore".
fn read_pasteboard_text() -> Result<String> {
    let pb = general_pasteboard()?;
    unsafe {
        let string_type = NSString::from_str(PB_TYPE_STRING);
        let result: Option<Retained<NSString>> = msg_send![&*pb, stringForType: &*string_type];
        match result {
            Some(s) => Ok(s.to_string()),
            None => Err(anyhow!("pasteboard has no string content")),
        }
    }
}

// ----- Modifier release wait ----------------------------------------------

/// Poll the combined session keyboard state until no modifier key is
/// down, capped at 600ms. Once released, sleep an additional 30ms to
/// give the OS a beat between the last modifier-up event and our
/// synthetic paste.
///
/// If the timeout fires, paste anyway (logged) — better to paste with
/// a stuck modifier than hang.
fn wait_for_modifiers_released() {
    let state = CGEventSourceStateID::CombinedSessionState;
    for _ in 0..MOD_POLL_MAX_ATTEMPTS {
        let any_held = MODIFIER_KEYCODES
            .iter()
            .any(|&k| unsafe { CGEventSourceKeyState(state, k) });
        if !any_held {
            thread::sleep(POST_RELEASE_DELAY);
            return;
        }
        thread::sleep(MOD_POLL_INTERVAL);
    }
    tracing::warn!("modifier-release wait timed out at 600ms; pasting anyway");
}
