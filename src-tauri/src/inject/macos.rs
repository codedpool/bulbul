//! macOS text injection: NSPasteboard clipboard + Cmd+V via enigo (CGEvent).
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
//! 5. Synthesize Cmd+V via [`enigo`] — Press(Meta) → Click(V) → wait
//!    100ms with Meta held so the target app processes the chord →
//!    Release(Meta). enigo posts the events through CGEventPost which
//!    only requires the Accessibility TCC permission already needed for
//!    our global-shortcut polling — no AppleEvents prompt.
//! 6. After 1s, restore the prior clipboard IF the clipboard still
//!    holds exactly what we wrote (so the user can copy something
//!    new mid-paste and we won't clobber it).
//!
//! Fallback: set `BULBUL_INJECT=osascript` in the environment to revert
//! to the prior AppleScript+System Events keystroke path. Keep around
//! until enough testers confirm enigo works on every Mac we care about.
//!
//! History: an earlier hand-rolled CGEvent path silently no-op'd on
//! some macOS versions, so the project moved to osascript (PR by
//! @Pskuntal1248). enigo's CGEvent path is the production path in
//! cjpais/handy (23k+ stars) and works across 10.15–Tahoe in the wild,
//! so we now use it as the default and keep osascript only as an opt-in
//! safety net.
//!
//! See:
//!   <https://github.com/nicke5012/TransientPasteboardType>
//!     — the convention clipboard managers honor for skip-record hints.

use anyhow::{anyhow, Context, Result};
use core_graphics::event_source::CGEventSourceStateID;
use enigo::{
    Direction::{Click, Press, Release},
    Enigo, Key, Keyboard, Settings,
};
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2_foundation::{NSArray, NSString};
use parking_lot::Mutex;
use std::process::Command;
use std::ptr;
use std::sync::OnceLock;
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

// ----- Mac virtual key codes for the keys we synthesize -------------------
// kVK_ANSI_C = 8, kVK_ANSI_V = 9. Layout-independent for the alpha block on
// a US-ANSI mapping; non-ANSI layouts (Dvorak, AZERTY) remap by character,
// but the C/V positions stay at kVK_ANSI_C/V because the shortcut is bound
// to the *position*, not the glyph — same way Cmd+V works in Dvorak Cocoa
// apps without rebinding.
const KVK_V: u16 = 9;
const KVK_C: u16 = 8;

// ----- Tuning constants. --------------------------------------------------
const MOD_POLL_INTERVAL: Duration = Duration::from_millis(25);
const MOD_POLL_MAX_ATTEMPTS: u32 = 24; // = 600ms ceiling
const POST_RELEASE_DELAY: Duration = Duration::from_millis(30);
const CLIPBOARD_SETTLE: Duration = Duration::from_millis(40);
const CLIPBOARD_RESTORE_DELAY: Duration = Duration::from_millis(1000);
/// How long Cmd stays held between press and release. Matches
/// cjpais/handy. Below ~60ms some Electron apps drop the chord; 100ms
/// is comfortably above that floor without being noticeable.
const CMD_HOLD: Duration = Duration::from_millis(100);

// ----- FFI: CGEventSourceKeyState ------------------------------------------
// CGEventSourceKeyState isn't bound by the Rust core-graphics crate. The
// C signature is documented as a pure-query function, safe from any
// thread. C99 `bool` is ABI-compatible with Rust's `bool`.
extern "C" {
    fn CGEventSourceKeyState(state_id: CGEventSourceStateID, key: u16) -> bool;
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

    send_cmd(KVK_V).context("posting Cmd+V")?;

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

/// Synthesize Cmd+V. Used by the Transform pipeline after writing
/// transformed text to the clipboard.
pub fn send_ctrl_v() -> Result<()> {
    send_cmd(KVK_V)
}

/// Synthesize Cmd+C. Used by the Transform pipeline to copy the user's
/// selection before processing.
pub fn send_ctrl_c() -> Result<()> {
    send_cmd(KVK_C)
}

// ----- Keystroke synthesis -------------------------------------------------

/// Dispatch a Cmd+<vkey> chord. Defaults to the enigo (CGEvent) path;
/// users can opt into the legacy osascript path with `BULBUL_INJECT=osascript`.
fn send_cmd(vkey: u16) -> Result<()> {
    if use_osascript_fallback() {
        let letter = match vkey {
            KVK_V => "v",
            KVK_C => "c",
            other => return Err(anyhow!("osascript fallback has no mapping for vkey {other}")),
        };
        return run_cmd_keystroke_osascript(letter);
    }
    send_cmd_enigo(vkey)
}

fn use_osascript_fallback() -> bool {
    std::env::var("BULBUL_INJECT")
        .map(|v| v.eq_ignore_ascii_case("osascript"))
        .unwrap_or(false)
}

/// Lazy global Enigo. `Enigo::new` allocates a CGEventSource and a few
/// other Core Graphics handles — cheap once, but we don't want to do it
/// per keystroke. Wrapped in a Mutex because `Keyboard::key` takes
/// `&mut self`. parking_lot is already a workspace dep so we get its
/// non-poisoning lock without dragging in a new crate.
fn enigo_handle() -> Result<&'static Mutex<Enigo>> {
    static SLOT: OnceLock<Mutex<Enigo>> = OnceLock::new();
    if let Some(m) = SLOT.get() {
        return Ok(m);
    }
    let e = Enigo::new(&Settings::default())
        .map_err(|e| anyhow!("Enigo::new failed (Accessibility permission?): {e}"))?;
    let _ = SLOT.set(Mutex::new(e));
    Ok(SLOT.get().expect("just-set OnceLock is Some"))
}

/// Press Cmd, click the given keycode, hold 100ms with Cmd still down,
/// then release Cmd. Mirrors cjpais/handy. The 100ms hold matters: some
/// Electron-based apps (Slack, VS Code at one point) drop the chord if
/// Cmd releases on the same frame the V keydown lands.
fn send_cmd_enigo(vkey: u16) -> Result<()> {
    let mu = enigo_handle()?;
    // Press + click are done while we hold the lock. We drop the lock
    // before sleeping so other paste paths aren't blocked for 100ms.
    {
        let mut e = mu.lock();
        e.key(Key::Meta, Press)
            .map_err(|err| anyhow!("press ⌘: {err}"))?;
        e.key(Key::Other(vkey as u32), Click)
            .map_err(|err| anyhow!("click vkey {vkey}: {err}"))?;
    }
    thread::sleep(CMD_HOLD);
    {
        let mut e = mu.lock();
        e.key(Key::Meta, Release)
            .map_err(|err| anyhow!("release ⌘: {err}"))?;
    }
    Ok(())
}

/// Drive `osascript` to send a single ⌘+key keystroke through System
/// Events. Legacy path kept as opt-in via `BULBUL_INJECT=osascript` for
/// any Mac where the enigo CGEvent path silently no-ops. Requires the
/// `NSAppleEventsUsageDescription` Info.plist key so TCC doesn't block
/// the AppleEvent send — the first call from a fresh install triggers
/// the "Bulbul wants to control System Events" prompt.
fn run_cmd_keystroke_osascript(key: &str) -> Result<()> {
    let script = format!(
        "tell application \"System Events\" to keystroke \"{key}\" using command down"
    );
    let status = Command::new("osascript")
        .args(["-e", &script])
        .status()
        .context("failed to spawn osascript")?;
    if !status.success() {
        return Err(anyhow!("osascript exited with status {status}"));
    }
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
