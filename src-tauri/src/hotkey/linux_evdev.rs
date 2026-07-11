//! Direct evdev keyboard reading — instant hold-to-talk on Linux.
//!
//! This is to the *hotkey* what uinput is to *typing*: it reads
//! `/dev/input/event*` below the compositor, so it works identically on
//! GNOME, KDE, wlroots, and X11, and — crucially — it sees the real key
//! DOWN and key UP. That gives true press-and-hold with no lag, no GNOME
//! custom shortcut, and no toggle. It's the default hotkey path whenever
//! we can read input devices.
//!
//! Access to input devices is the `input` group (the same grant the
//! .deb sets up for uinput typing, one relogin). When we can't read any
//! keyboard, `available()` is false and the caller falls back to the
//! portal / custom-shortcut path.
//!
//! Scope: keyed hotkeys only (a chord that ends in a real key, e.g.
//! Ctrl+Alt+Space). That covers every Linux default and preset. A
//! modifier-only chord (Ctrl+Win) can't be debounced with a pure
//! blocking read — those aren't offered on Linux and fall back if set.
//!
//! One reader thread per keyboard. State is per-device because a
//! physical keyboard emits its own modifiers and key together. Threads
//! from a superseded registration exit on their next key event (a
//! generation counter); brief overlap is harmless because the
//! orchestrator ignores press-while-active and release-while-idle.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use evdev::{Device, EventType, KeyCode};

use super::{HotkeyEvent, ParsedHotkey, FIRE_COOLDOWN_MS};

/// Bumped on every register()/stop(). Reader threads compare their
/// captured value against this and exit when superseded.
static GENERATION: AtomicU64 = AtomicU64::new(0);

fn is_keyboard(d: &Device) -> bool {
    d.supported_keys().is_some_and(|keys| {
        keys.contains(KeyCode::KEY_SPACE) && keys.contains(KeyCode::KEY_LEFTCTRL)
    })
}

fn keyboards() -> Vec<(std::path::PathBuf, Device)> {
    // enumerate() silently skips devices it can't open, so with no input
    // access this yields nothing — exactly our "not granted yet" signal.
    evdev::enumerate().filter(|(_, d)| is_keyboard(d)).collect()
}

/// True when at least one keyboard device is readable (i.e. we're in the
/// input group). Cheap enough to call per registration.
pub fn available() -> bool {
    !keyboards().is_empty()
}

/// Stop all reader threads from the current registration (they exit on
/// their next key event).
pub fn stop() {
    GENERATION.fetch_add(1, Ordering::SeqCst);
}

/// A hotkey we watch, resolved to evdev codes. `key` is the required
/// non-modifier key; modifier-only hotkeys are dropped before we get
/// here (see `resolve`).
struct Spec {
    ctrl: bool,
    shift: bool,
    alt: bool,
    meta: bool,
    key: u16,
    pressed: HotkeyEvent,
    /// Fired on chord release. `None` for tap-to-trigger hotkeys
    /// (transform slots), which act on press only.
    released: Option<HotkeyEvent>,
}

fn resolve(hk: &ParsedHotkey, pressed: HotkeyEvent, released: Option<HotkeyEvent>) -> Option<Spec> {
    let key = hk.key.as_deref().and_then(key_name_to_evdev)?.code();
    Some(Spec {
        ctrl: hk.ctrl,
        shift: hk.shift,
        alt: hk.alt,
        meta: hk.meta,
        key,
        pressed,
        released,
    })
}

fn chord_held(spec: &Spec, held: &HashSet<u16>) -> bool {
    let ok = |need: bool, l: u16, r: u16| !need || held.contains(&l) || held.contains(&r);
    ok(spec.ctrl, C_LCTRL, C_RCTRL)
        && ok(spec.shift, C_LSHIFT, C_RSHIFT)
        && ok(spec.alt, C_LALT, C_RALT)
        && ok(spec.meta, C_LMETA, C_RMETA)
        && held.contains(&spec.key)
}

/// Register dictation + polish + transform slots for direct reading.
/// Replaces any previous registration. Returns the transform ids that
/// resolved to a watchable chord, so the caller can report slot status to
/// the UI (the evdev reader replaces the global-shortcut plugin on
/// Wayland, where the plugin can't see keys). Non-fatal: if nothing
/// resolves or no keyboards are readable, it just doesn't start (the
/// caller already checked `available()`).
pub fn register(
    tx: Sender<HotkeyEvent>,
    dictation: ParsedHotkey,
    polish: ParsedHotkey,
    transforms: &[(i64, ParsedHotkey)],
) -> Vec<i64> {
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;

    let mut specs: Vec<Spec> = Vec::new();
    if let Some(s) = resolve(
        &dictation,
        HotkeyEvent::DictationPressed,
        Some(HotkeyEvent::DictationReleased),
    ) {
        specs.push(s);
    }
    if let Some(s) = resolve(
        &polish,
        HotkeyEvent::PolishDictationPressed,
        Some(HotkeyEvent::PolishDictationReleased),
    ) {
        specs.push(s);
    }
    // Transform slots are tap-to-trigger: fire TransformTriggered(id) on
    // press, nothing on release (released = None). The FIRE_COOLDOWN in
    // reader_loop debounces auto-repeat while the chord is held.
    let mut registered_ids: Vec<i64> = Vec::new();
    for (id, hk) in transforms {
        if let Some(s) = resolve(hk, HotkeyEvent::TransformTriggered(*id), None) {
            specs.push(s);
            registered_ids.push(*id);
        }
    }
    if specs.is_empty() {
        tracing::warn!("evdev: no keyed hotkey to watch (modifier-only?)");
        return registered_ids;
    }
    let specs = Arc::new(specs);

    let devices = keyboards();
    if devices.is_empty() {
        return registered_ids;
    }
    let count = devices.len();
    for (path, device) in devices {
        let tx = tx.clone();
        let specs = specs.clone();
        thread::Builder::new()
            .name("bulbul-evdev".into())
            .spawn(move || reader_loop(device, path, specs, tx, generation))
            .ok();
    }
    tracing::info!(
        "evdev hotkey watcher started on {count} keyboard(s), {} transform slot(s)",
        registered_ids.len()
    );
    crate::linux_env::emit_hotkey_status(
        "evdev",
        "Reading the keyboard directly — instant hold-to-talk.".to_string(),
    );
    registered_ids
}

fn reader_loop(
    mut device: Device,
    path: std::path::PathBuf,
    specs: Arc<Vec<Spec>>,
    tx: Sender<HotkeyEvent>,
    generation: u64,
) {
    let mut held: HashSet<u16> = HashSet::new();
    // Per-spec: is the chord currently firing, and when it last fired.
    let mut firing = vec![false; specs.len()];
    let mut last_fire: Vec<Option<Instant>> = vec![None; specs.len()];

    loop {
        let events = match device.fetch_events() {
            Ok(e) => e,
            Err(e) => {
                tracing::debug!("evdev reader for {path:?} ending: {e}");
                return;
            }
        };
        // A superseded registration exits here, on the next key activity.
        if GENERATION.load(Ordering::SeqCst) != generation {
            return;
        }
        for ev in events {
            if ev.event_type() != EventType::KEY {
                continue;
            }
            let code = ev.code();
            match ev.value() {
                0 => {
                    held.remove(&code);
                }
                1 | 2 => {
                    held.insert(code);
                }
                _ => {}
            }
            // Evaluate every spec after each key transition so a quick
            // tap still produces a clean press→release pair.
            for (i, spec) in specs.iter().enumerate() {
                let now_held = chord_held(spec, &held);
                if now_held && !firing[i] {
                    let cooled = last_fire[i]
                        .map_or(true, |t| t.elapsed().as_millis() >= FIRE_COOLDOWN_MS);
                    if cooled {
                        let _ = tx.send(spec.pressed.clone());
                        last_fire[i] = Some(Instant::now());
                        firing[i] = true;
                    }
                } else if !now_held && firing[i] {
                    if let Some(ev) = &spec.released {
                        let _ = tx.send(ev.clone());
                    }
                    firing[i] = false;
                }
            }
        }
    }
}

// --- evdev modifier keycodes (layout-independent) --------------------------
const C_LCTRL: u16 = KeyCode::KEY_LEFTCTRL.0;
const C_RCTRL: u16 = KeyCode::KEY_RIGHTCTRL.0;
const C_LSHIFT: u16 = KeyCode::KEY_LEFTSHIFT.0;
const C_RSHIFT: u16 = KeyCode::KEY_RIGHTSHIFT.0;
const C_LALT: u16 = KeyCode::KEY_LEFTALT.0;
const C_RALT: u16 = KeyCode::KEY_RIGHTALT.0;
const C_LMETA: u16 = KeyCode::KEY_LEFTMETA.0;
const C_RMETA: u16 = KeyCode::KEY_RIGHTMETA.0;

/// Map a Bulbul key-name (the normalized form from `normalize_key_name`)
/// to an evdev KeyCode. Covers everything the recorder can produce.
fn key_name_to_evdev(name: &str) -> Option<KeyCode> {
    Some(match name {
        "A" => KeyCode::KEY_A, "B" => KeyCode::KEY_B, "C" => KeyCode::KEY_C,
        "D" => KeyCode::KEY_D, "E" => KeyCode::KEY_E, "F" => KeyCode::KEY_F,
        "G" => KeyCode::KEY_G, "H" => KeyCode::KEY_H, "I" => KeyCode::KEY_I,
        "J" => KeyCode::KEY_J, "K" => KeyCode::KEY_K, "L" => KeyCode::KEY_L,
        "M" => KeyCode::KEY_M, "N" => KeyCode::KEY_N, "O" => KeyCode::KEY_O,
        "P" => KeyCode::KEY_P, "Q" => KeyCode::KEY_Q, "R" => KeyCode::KEY_R,
        "S" => KeyCode::KEY_S, "T" => KeyCode::KEY_T, "U" => KeyCode::KEY_U,
        "V" => KeyCode::KEY_V, "W" => KeyCode::KEY_W, "X" => KeyCode::KEY_X,
        "Y" => KeyCode::KEY_Y, "Z" => KeyCode::KEY_Z,
        "0" => KeyCode::KEY_0, "1" => KeyCode::KEY_1, "2" => KeyCode::KEY_2,
        "3" => KeyCode::KEY_3, "4" => KeyCode::KEY_4, "5" => KeyCode::KEY_5,
        "6" => KeyCode::KEY_6, "7" => KeyCode::KEY_7, "8" => KeyCode::KEY_8,
        "9" => KeyCode::KEY_9,
        "Space" => KeyCode::KEY_SPACE, "Tab" => KeyCode::KEY_TAB,
        "Enter" | "Return" => KeyCode::KEY_ENTER,
        "Backspace" => KeyCode::KEY_BACKSPACE, "Escape" => KeyCode::KEY_ESC,
        "F1" => KeyCode::KEY_F1, "F2" => KeyCode::KEY_F2, "F3" => KeyCode::KEY_F3,
        "F4" => KeyCode::KEY_F4, "F5" => KeyCode::KEY_F5, "F6" => KeyCode::KEY_F6,
        "F7" => KeyCode::KEY_F7, "F8" => KeyCode::KEY_F8, "F9" => KeyCode::KEY_F9,
        "F10" => KeyCode::KEY_F10, "F11" => KeyCode::KEY_F11, "F12" => KeyCode::KEY_F12,
        "Up" => KeyCode::KEY_UP, "Down" => KeyCode::KEY_DOWN,
        "Left" => KeyCode::KEY_LEFT, "Right" => KeyCode::KEY_RIGHT,
        "Home" => KeyCode::KEY_HOME, "End" => KeyCode::KEY_END,
        "PageUp" => KeyCode::KEY_PAGEUP, "PageDown" => KeyCode::KEY_PAGEDOWN,
        "Insert" => KeyCode::KEY_INSERT, "Delete" => KeyCode::KEY_DELETE,
        ";" => KeyCode::KEY_SEMICOLON, "'" => KeyCode::KEY_APOSTROPHE,
        "," => KeyCode::KEY_COMMA, "." => KeyCode::KEY_DOT, "/" => KeyCode::KEY_SLASH,
        "\\" => KeyCode::KEY_BACKSLASH, "[" => KeyCode::KEY_LEFTBRACE,
        "]" => KeyCode::KEY_RIGHTBRACE, "-" => KeyCode::KEY_MINUS,
        "=" => KeyCode::KEY_EQUAL, "`" => KeyCode::KEY_GRAVE,
        _ => return None,
    })
}
