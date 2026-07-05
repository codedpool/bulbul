//! Kernel uinput virtual-keyboard injection.
//!
//! This is the injection path that actually works on GNOME Wayland.
//! Mutter blocks the clean Wayland typing protocols (wtype) and silently
//! drops the RemoteDesktop portal's Notify* input — but uinput creates a
//! virtual keyboard *in the kernel*, below the compositor, so the events
//! are indistinguishable from a real keyboard and every compositor
//! honors them. It's the mechanism Speech Note, nerd-dictation, numen,
//! and voxtype all end up using.
//!
//! The only cost is access to `/dev/uinput`, which is root:root 0660 by
//! default. The .deb grants it by adding the user to the standard
//! `input` group and installing a udev rule that gives that group the
//! device — so after one log out/in Bulbul can open uinput with no
//! broad capability and no setuid/setgid binary. See `deb/postinst.sh`.
//!
//! We keep one virtual device alive for the process lifetime (a uinput
//! device vanishes when its handle drops). It can both tap a chord
//! (Ctrl+V / Ctrl+C for clipboard paste) and type text directly, one
//! key at a time. Direct typing skips the clipboard entirely — no
//! save/paste/restore round-trip — which is faster and free of the brief
//! screen flicker a clipboard swap can cause. It maps characters against
//! a US-QWERTY layout; text with characters outside that map (emoji,
//! accents) is left to the clipboard path, which is layout- and
//! Unicode-safe.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use evdev::{uinput::VirtualDevice, AttributeSet, EventType, InputEvent, KeyCode};
use parking_lot::Mutex;

pub const KEY_V: KeyCode = KeyCode::KEY_V;
pub const KEY_C: KeyCode = KeyCode::KEY_C;
const KEY_LEFTCTRL: KeyCode = KeyCode::KEY_LEFTCTRL;
const KEY_LEFTSHIFT: KeyCode = KeyCode::KEY_LEFTSHIFT;

const PRESS: i32 = 1;
const RELEASE: i32 = 0;
// Small gap between synthetic events. Some apps debounce or miss a chord
// whose press/release land in the same kernel tick; a couple ms each
// makes the combo read as deliberate without adding perceptible latency.
const EVENT_GAP: Duration = Duration::from_millis(4);
// Per-key gap while typing a transcript. Smaller than EVENT_GAP because
// we emit many keys back-to-back; a couple ms keeps fast typists' worth
// of events from coalescing without making a sentence feel slow.
const TYPE_GAP: Duration = Duration::from_millis(2);

static DEVICE: OnceLock<Mutex<Option<VirtualDevice>>> = OnceLock::new();
static READY: AtomicBool = AtomicBool::new(false);

fn slot() -> &'static Mutex<Option<VirtualDevice>> {
    DEVICE.get_or_init(|| Mutex::new(None))
}

/// True once a virtual keyboard has been created and can emit.
pub fn is_ready() -> bool {
    READY.load(Ordering::Relaxed)
}

/// Try to create the virtual keyboard. Fails (without panicking) when we
/// lack access to /dev/uinput — the caller treats that as "fall back and
/// tell the user how to grant access." Idempotent: a second successful
/// call replaces the device; a failure leaves any existing one intact.
pub fn init() -> Result<()> {
    let mut keys = AttributeSet::<KeyCode>::new();
    // Modifiers + the chord keys, plus every key the typing map can
    // produce — a uinput device rejects emit() for a key it never
    // declared, so the set has to cover the whole US layout we type from.
    keys.insert(KEY_LEFTCTRL);
    keys.insert(KEY_LEFTSHIFT);
    for kc in TYPING_KEYS {
        keys.insert(kc);
    }

    let device = VirtualDevice::builder()
        .context("opening /dev/uinput (no access yet?)")?
        .name("Bulbul Virtual Keyboard")
        .with_keys(&keys)
        .context("declaring virtual keyboard keys")?
        .build()
        .context("building uinput virtual device")?;

    *slot().lock() = Some(device);
    READY.store(true, Ordering::Relaxed);
    tracing::info!("uinput virtual keyboard ready");
    Ok(())
}

/// Emit Ctrl+<key> through the virtual keyboard. Returns Err if the
/// device isn't initialized or the kernel write fails — caller falls
/// back to the portal/tool chain.
pub fn send_combo(key: KeyCode) -> Result<()> {
    let mut guard = slot().lock();
    let device = guard
        .as_mut()
        .ok_or_else(|| anyhow!("uinput device not initialized"))?;

    // Ctrl down, key down, key up, Ctrl up — evdev appends a SYN report
    // after each emit() so the compositor sees each transition.
    emit(device, KEY_LEFTCTRL, PRESS)?;
    std::thread::sleep(EVENT_GAP);
    emit(device, key, PRESS)?;
    std::thread::sleep(EVENT_GAP);
    emit(device, key, RELEASE)?;
    std::thread::sleep(EVENT_GAP);
    emit(device, KEY_LEFTCTRL, RELEASE)?;
    Ok(())
}

/// Type `text` character by character through the virtual keyboard —
/// no clipboard involved. Returns Err *without emitting anything* if the
/// device isn't ready or the text contains a character we can't produce
/// on a US layout, so the caller can fall back to clipboard paste for
/// the whole string (no half-typed duplication).
pub fn type_text(text: &str) -> Result<()> {
    // Pre-resolve every character first. If any is off-map, bail before
    // touching the keyboard so nothing is partially typed.
    let mut plan = Vec::with_capacity(text.len());
    for c in text.chars() {
        match char_to_key(c) {
            Some(entry) => plan.push(entry),
            None => return Err(anyhow!("character {c:?} is not on the US layout map")),
        }
    }

    let mut guard = slot().lock();
    let device = guard
        .as_mut()
        .ok_or_else(|| anyhow!("uinput device not initialized"))?;

    for (key, shift) in plan {
        if shift {
            emit(device, KEY_LEFTSHIFT, PRESS)?;
            std::thread::sleep(TYPE_GAP);
        }
        emit(device, key, PRESS)?;
        std::thread::sleep(TYPE_GAP);
        emit(device, key, RELEASE)?;
        std::thread::sleep(TYPE_GAP);
        if shift {
            emit(device, KEY_LEFTSHIFT, RELEASE)?;
            std::thread::sleep(TYPE_GAP);
        }
    }
    Ok(())
}

fn emit(device: &mut VirtualDevice, key: KeyCode, val: i32) -> Result<()> {
    // evdev 0.13 has no KeyEvent helper — build the raw InputEvent
    // (KEY event type, the key's scancode, press/release value).
    let ev = InputEvent::new(EventType::KEY.0, key.code(), val);
    device
        .emit(&[ev])
        .with_context(|| format!("emitting {key:?}={val}"))?;
    Ok(())
}

/// Every key the typing map can reach, so `init` can declare them.
const TYPING_KEYS: [KeyCode; 50] = [
    KeyCode::KEY_A, KeyCode::KEY_B, KeyCode::KEY_C, KeyCode::KEY_D,
    KeyCode::KEY_E, KeyCode::KEY_F, KeyCode::KEY_G, KeyCode::KEY_H,
    KeyCode::KEY_I, KeyCode::KEY_J, KeyCode::KEY_K, KeyCode::KEY_L,
    KeyCode::KEY_M, KeyCode::KEY_N, KeyCode::KEY_O, KeyCode::KEY_P,
    KeyCode::KEY_Q, KeyCode::KEY_R, KeyCode::KEY_S, KeyCode::KEY_T,
    KeyCode::KEY_U, KeyCode::KEY_V, KeyCode::KEY_W, KeyCode::KEY_X,
    KeyCode::KEY_Y, KeyCode::KEY_Z,
    KeyCode::KEY_1, KeyCode::KEY_2, KeyCode::KEY_3, KeyCode::KEY_4,
    KeyCode::KEY_5, KeyCode::KEY_6, KeyCode::KEY_7, KeyCode::KEY_8,
    KeyCode::KEY_9, KeyCode::KEY_0,
    KeyCode::KEY_MINUS, KeyCode::KEY_EQUAL, KeyCode::KEY_LEFTBRACE,
    KeyCode::KEY_RIGHTBRACE, KeyCode::KEY_BACKSLASH, KeyCode::KEY_SEMICOLON,
    KeyCode::KEY_APOSTROPHE, KeyCode::KEY_GRAVE, KeyCode::KEY_COMMA,
    KeyCode::KEY_DOT, KeyCode::KEY_SLASH,
    KeyCode::KEY_SPACE, KeyCode::KEY_ENTER, KeyCode::KEY_TAB,
];

/// Map a character to the US-QWERTY key that produces it and whether
/// Shift is held. `None` means "not typeable on this layout" — the
/// caller then routes the whole string through the clipboard instead.
fn char_to_key(c: char) -> Option<(KeyCode, bool)> {
    use evdev::KeyCode as K;
    Some(match c {
        'a' => (K::KEY_A, false), 'A' => (K::KEY_A, true),
        'b' => (K::KEY_B, false), 'B' => (K::KEY_B, true),
        'c' => (K::KEY_C, false), 'C' => (K::KEY_C, true),
        'd' => (K::KEY_D, false), 'D' => (K::KEY_D, true),
        'e' => (K::KEY_E, false), 'E' => (K::KEY_E, true),
        'f' => (K::KEY_F, false), 'F' => (K::KEY_F, true),
        'g' => (K::KEY_G, false), 'G' => (K::KEY_G, true),
        'h' => (K::KEY_H, false), 'H' => (K::KEY_H, true),
        'i' => (K::KEY_I, false), 'I' => (K::KEY_I, true),
        'j' => (K::KEY_J, false), 'J' => (K::KEY_J, true),
        'k' => (K::KEY_K, false), 'K' => (K::KEY_K, true),
        'l' => (K::KEY_L, false), 'L' => (K::KEY_L, true),
        'm' => (K::KEY_M, false), 'M' => (K::KEY_M, true),
        'n' => (K::KEY_N, false), 'N' => (K::KEY_N, true),
        'o' => (K::KEY_O, false), 'O' => (K::KEY_O, true),
        'p' => (K::KEY_P, false), 'P' => (K::KEY_P, true),
        'q' => (K::KEY_Q, false), 'Q' => (K::KEY_Q, true),
        'r' => (K::KEY_R, false), 'R' => (K::KEY_R, true),
        's' => (K::KEY_S, false), 'S' => (K::KEY_S, true),
        't' => (K::KEY_T, false), 'T' => (K::KEY_T, true),
        'u' => (K::KEY_U, false), 'U' => (K::KEY_U, true),
        'v' => (K::KEY_V, false), 'V' => (K::KEY_V, true),
        'w' => (K::KEY_W, false), 'W' => (K::KEY_W, true),
        'x' => (K::KEY_X, false), 'X' => (K::KEY_X, true),
        'y' => (K::KEY_Y, false), 'Y' => (K::KEY_Y, true),
        'z' => (K::KEY_Z, false), 'Z' => (K::KEY_Z, true),
        '1' => (K::KEY_1, false), '!' => (K::KEY_1, true),
        '2' => (K::KEY_2, false), '@' => (K::KEY_2, true),
        '3' => (K::KEY_3, false), '#' => (K::KEY_3, true),
        '4' => (K::KEY_4, false), '$' => (K::KEY_4, true),
        '5' => (K::KEY_5, false), '%' => (K::KEY_5, true),
        '6' => (K::KEY_6, false), '^' => (K::KEY_6, true),
        '7' => (K::KEY_7, false), '&' => (K::KEY_7, true),
        '8' => (K::KEY_8, false), '*' => (K::KEY_8, true),
        '9' => (K::KEY_9, false), '(' => (K::KEY_9, true),
        '0' => (K::KEY_0, false), ')' => (K::KEY_0, true),
        '-' => (K::KEY_MINUS, false), '_' => (K::KEY_MINUS, true),
        '=' => (K::KEY_EQUAL, false), '+' => (K::KEY_EQUAL, true),
        '[' => (K::KEY_LEFTBRACE, false), '{' => (K::KEY_LEFTBRACE, true),
        ']' => (K::KEY_RIGHTBRACE, false), '}' => (K::KEY_RIGHTBRACE, true),
        '\\' => (K::KEY_BACKSLASH, false), '|' => (K::KEY_BACKSLASH, true),
        ';' => (K::KEY_SEMICOLON, false), ':' => (K::KEY_SEMICOLON, true),
        '\'' => (K::KEY_APOSTROPHE, false), '"' => (K::KEY_APOSTROPHE, true),
        '`' => (K::KEY_GRAVE, false), '~' => (K::KEY_GRAVE, true),
        ',' => (K::KEY_COMMA, false), '<' => (K::KEY_COMMA, true),
        '.' => (K::KEY_DOT, false), '>' => (K::KEY_DOT, true),
        '/' => (K::KEY_SLASH, false), '?' => (K::KEY_SLASH, true),
        ' ' => (K::KEY_SPACE, false),
        '\n' => (K::KEY_ENTER, false),
        '\t' => (K::KEY_TAB, false),
        _ => return None,
    })
}
