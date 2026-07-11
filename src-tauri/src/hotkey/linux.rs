//! Linux-native hotkey primitives (X11 path).
//!
//! Polling-based, mirroring the Windows + Mac architecture. A dedicated
//! background thread queries the global keyboard state via x11rb's
//! `query_keymap` every 25ms and runs the same state machine as the
//! other platforms (idle → arming → pressing → idle).
//!
//! Each watcher/poller spawns its own X11 connection and holds it
//! across the polling lifetime — `x11rb::connect` is ~5-10ms and we
//! don't want to pay that every 25ms tick. The connection is `Send +
//! Sync` so it lives happily inside the spawned thread.
//!
//! Wayland: this file is the X11 path. On pure-Wayland sessions
//! without XWayland, x11rb::connect fails and the watcher fires an
//! immediate release (same shape as the Phase 0 stub). A future
//! follow-up adds a portal-based Wayland watcher that calls
//! `org.freedesktop.portal.GlobalShortcuts` for press AND release.
//! Until then, users on pure Wayland fall back to XWayland or set
//! `BULBUL_FORCE_X11=1`.
//!
//! Keycodes hard-coded for US QWERTY (modifiers are layout-independent;
//! the typing keys aren't — Phase 7 polish can add XKB-based lookup).

use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::xproto::ConnectionExt;
use x11rb::rust_connection::RustConnection;

use super::{HotkeyEvent, ParsedHotkey, FIRE_COOLDOWN_MS};

const RELEASE_POLL_MS: u64 = 25;
const MODIFIER_CHORD_DEBOUNCE_MS: u64 = 80;
const RELEASE_POLL_TIMEOUT_SECS: u64 = 60;

// X11 protocol-level keycodes. Modifier codes are layout-independent
// (X11's modifier mapping doesn't change with the keyboard layout); the
// typing-key codes assume US QWERTY.
const KC_CONTROL_L: u8 = 37;
const KC_CONTROL_R: u8 = 105;
const KC_SHIFT_L: u8 = 50;
const KC_SHIFT_R: u8 = 62;
const KC_ALT_L: u8 = 64;
const KC_ALT_R: u8 = 108;
const KC_SUPER_L: u8 = 133;
const KC_SUPER_R: u8 = 134;

/// Map a hotkey key-name string to an X11 keycode (US QWERTY positions).
/// Hard-coded for a US-ANSI PC keyboard with the standard xkb evdev
/// mapping. Mirrors `super::key_name_to_code` so the release poller
/// can resolve every key the recorder accepts — without these new
/// entries, binding e.g. `Ctrl+Shift+;` succeeds at press but
/// `DictationReleased` fires immediately and dictation captures
/// nothing.
fn key_name_to_x11_keycode(name: &str) -> Option<u8> {
    Some(match name {
        "A" => 38, "B" => 56, "C" => 54, "D" => 40, "E" => 26, "F" => 41,
        "G" => 42, "H" => 43, "I" => 31, "J" => 44, "K" => 45, "L" => 46,
        "M" => 58, "N" => 57, "O" => 32, "P" => 33, "Q" => 24, "R" => 27,
        "S" => 39, "T" => 28, "U" => 30, "V" => 55, "W" => 25, "X" => 53,
        "Y" => 29, "Z" => 52,
        "0" => 19, "1" => 10, "2" => 11, "3" => 12, "4" => 13,
        "5" => 14, "6" => 15, "7" => 16, "8" => 17, "9" => 18,
        "Space" => 65, "Tab" => 23, "Return" | "Enter" => 36,
        "Backspace" => 22, "Escape" => 9,
        "F1" => 67, "F2" => 68, "F3" => 69, "F4" => 70, "F5" => 71,
        "F6" => 72, "F7" => 73, "F8" => 74, "F9" => 75, "F10" => 76,
        "F11" => 95, "F12" => 96,
        // Arrows.
        "Up" => 111, "Down" => 116, "Left" => 113, "Right" => 114,
        // Navigation block.
        "Home" => 110, "End" => 115, "PageUp" => 112, "PageDown" => 117,
        "Insert" => 118, "Delete" => 119,
        // Punctuation (xkb evdev).
        ";" => 47, "'" => 48, "," => 59, "." => 60, "/" => 61,
        "\\" => 51, "[" => 34, "]" => 35,
        "-" => 20, "=" => 21, "`" => 49,
        _ => return None,
    })
}

/// Read the 256-bit keyboard bitmap from the X server. Returns None if
/// the request fails (server disconnected, etc).
fn query_keymap(conn: &RustConnection) -> Option<[u8; 32]> {
    let reply = conn.query_keymap().ok()?.reply().ok()?;
    Some(reply.keys)
}

fn key_in_keymap(keymap: &[u8; 32], keycode: u8) -> bool {
    let byte = (keycode as usize) / 8;
    let bit = (keycode as usize) % 8;
    (keymap[byte] >> bit) & 1 != 0
}

/// Returns (ctrl, shift, alt, super) from a keymap snapshot. L+R OR'd.
fn modifier_state_from(keymap: &[u8; 32]) -> (bool, bool, bool, bool) {
    (
        key_in_keymap(keymap, KC_CONTROL_L) || key_in_keymap(keymap, KC_CONTROL_R),
        key_in_keymap(keymap, KC_SHIFT_L) || key_in_keymap(keymap, KC_SHIFT_R),
        key_in_keymap(keymap, KC_ALT_L) || key_in_keymap(keymap, KC_ALT_R),
        key_in_keymap(keymap, KC_SUPER_L) || key_in_keymap(keymap, KC_SUPER_R),
    )
}

fn required_mods_held(need: &ParsedHotkey, state: (bool, bool, bool, bool)) -> bool {
    let (ctrl, shift, alt, sup) = state;
    (!need.ctrl || ctrl)
        && (!need.shift || shift)
        && (!need.alt || alt)
        && (!need.meta || sup)
}

fn modifier_chord_alive_slot() -> &'static Mutex<Option<Arc<AtomicBool>>> {
    static SLOT: OnceLock<Mutex<Option<Arc<AtomicBool>>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

pub fn stop_native_watchers() {
    let mut slot = modifier_chord_alive_slot().lock();
    if let Some(prev) = slot.take() {
        prev.store(false, Ordering::Relaxed);
    }
}

pub fn spawn_modifier_chord_watcher(tx: Sender<HotkeyEvent>, hotkey: ParsedHotkey) {
    let alive = Arc::new(AtomicBool::new(true));
    *modifier_chord_alive_slot().lock() = Some(alive.clone());

    thread::spawn(move || {
        let (conn, _) = match x11rb::connect(None) {
            Ok(c) => c,
            Err(e) => {
                // No X server at all (pure Wayland without XWayland, or
                // headless). Tell the user instead of dying silently —
                // this was the "hotkey just doesn't work" black hole.
                tracing::warn!(
                    "Linux modifier-chord watcher: x11rb::connect failed: {e}; \
                     hotkey {:?} will not fire",
                    hotkey
                );
                crate::linux_env::emit_hotkey_status(
                    "none",
                    "Couldn't reach an X server to watch the keyboard. \
                     Bind a system shortcut to Bulbul's CLI toggle instead."
                        .to_string(),
                );
                return;
            }
        };

        enum State {
            Idle,
            Arming(Instant),
            Pressing,
        }
        let mut state = State::Idle;
        let mut last_fire: Option<Instant> = None;
        tracing::info!("Linux modifier-chord watcher started for {:?}", hotkey);
        crate::linux_env::emit_hotkey_status(
            "x11",
            "Watching the keyboard via X11.".to_string(),
        );

        while alive.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(RELEASE_POLL_MS));
            let Some(keymap) = query_keymap(&conn) else { continue; };
            let held = required_mods_held(&hotkey, modifier_state_from(&keymap));
            match state {
                State::Idle => {
                    if held {
                        let cooled = last_fire
                            .map_or(true, |t| t.elapsed().as_millis() >= FIRE_COOLDOWN_MS);
                        if cooled {
                            state = State::Arming(Instant::now());
                        }
                    }
                }
                State::Arming(start) => {
                    if !held {
                        state = State::Idle;
                    } else if start.elapsed().as_millis()
                        >= MODIFIER_CHORD_DEBOUNCE_MS as u128
                    {
                        tracing::debug!("Linux modifier-chord dictation pressed: {:?}", hotkey);
                        let _ = tx.send(HotkeyEvent::DictationPressed);
                        last_fire = Some(Instant::now());
                        state = State::Pressing;
                    }
                }
                State::Pressing => {
                    if !held {
                        let _ = tx.send(HotkeyEvent::DictationReleased);
                        state = State::Idle;
                    }
                }
            }
        }
        if matches!(state, State::Pressing) {
            let _ = tx.send(HotkeyEvent::DictationReleased);
        }
        tracing::info!("Linux modifier-chord watcher exited for {:?}", hotkey);
    });
}

pub fn spawn_release_poller(
    tx: Sender<HotkeyEvent>,
    hotkey: ParsedHotkey,
    release_evt: HotkeyEvent,
) {
    let Some(main_kc) = hotkey.key.as_deref().and_then(key_name_to_x11_keycode) else {
        tracing::warn!(
            "Linux release poller: no main key for hotkey {:?}; firing immediate release",
            hotkey
        );
        let _ = tx.send(release_evt);
        return;
    };

    thread::spawn(move || {
        let (conn, _) = match x11rb::connect(None) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(
                    "Linux release poller: x11rb::connect failed: {e}; firing immediate release"
                );
                let _ = tx.send(release_evt);
                return;
            }
        };

        let started = Instant::now();
        loop {
            thread::sleep(Duration::from_millis(RELEASE_POLL_MS));
            if started.elapsed() > Duration::from_secs(RELEASE_POLL_TIMEOUT_SECS) {
                tracing::warn!("Linux release poller timed out — forcing release");
                let _ = tx.send(release_evt.clone());
                return;
            }
            let Some(keymap) = query_keymap(&conn) else { continue; };
            let main_down = key_in_keymap(&keymap, main_kc);
            let state = modifier_state_from(&keymap);
            if !main_down || !required_mods_held(&hotkey, state) {
                let _ = tx.send(release_evt.clone());
                return;
            }
        }
    });
}
