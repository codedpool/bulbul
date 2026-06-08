//! macOS-native hotkey primitives.
//!
//! Polling-based, mirroring the Windows architecture: a dedicated
//! background thread queries live key state via CGEventSourceKeyState
//! every 25ms and runs the same state machine as
//! [`super::windows`] (idle → arming → pressing → idle).
//!
//! The tauri-plugin-global-shortcut crate already handles PRESS
//! detection for regular combos on Mac (it uses Carbon
//! RegisterEventHotKey under the hood); this file only owns the
//! polling-based release detector and the modifier-only chord watcher
//! that the plugin can't represent.
//!
//! Modifier mapping: ParsedHotkey `meta` → Command (⌘),
//! `alt` → Option (⌥), `shift` → ⇧, `ctrl` → ⌃. Both physical L/R
//! modifier keys are OR'd together when checking state.

use core_graphics::event_source::CGEventSourceStateID;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use super::{HotkeyEvent, ParsedHotkey, FIRE_COOLDOWN_MS};

/// How often the release poller checks live key state.
const RELEASE_POLL_MS: u64 = 25;
/// How long both modifiers of a modifier-only chord must be held before
/// we treat it as a dictation request.
const MODIFIER_CHORD_DEBOUNCE_MS: u64 = 80;
/// Safety net for the release poller. Mirrors the Windows ceiling.
const RELEASE_POLL_TIMEOUT_SECS: u64 = 60;

// CGEventSourceKeyState isn't bound by core-graphics' Rust crate. Same
// declaration as in inject/macos.rs; duplicated rather than shared since
// cross-module visibility on a private FFI symbol is more friction than
// keeping two five-line blocks in sync. C signature:
//   bool CGEventSourceKeyState(CGEventSourceStateID stateID, CGKeyCode key);
extern "C" {
    fn CGEventSourceKeyState(state_id: CGEventSourceStateID, key: u16) -> bool;
}

// Mac virtual key codes for modifiers. L+R because the physical keyboard
// exposes both separately and the user may be holding either.
const KC_CMD_L: u16 = 55;
const KC_CMD_R: u16 = 54;
const KC_SHIFT_L: u16 = 56;
const KC_SHIFT_R: u16 = 60;
const KC_OPTION_L: u16 = 58;
const KC_OPTION_R: u16 = 61;
const KC_CONTROL_L: u16 = 59;
const KC_CONTROL_R: u16 = 62;

fn is_key_down(keycode: u16) -> bool {
    unsafe { CGEventSourceKeyState(CGEventSourceStateID::CombinedSessionState, keycode) }
}

/// Returns (ctrl, shift, option, command) — the live held-state of each
/// modifier family. L+R variants OR'd together.
fn modifier_state() -> (bool, bool, bool, bool) {
    (
        is_key_down(KC_CONTROL_L) || is_key_down(KC_CONTROL_R),
        is_key_down(KC_SHIFT_L) || is_key_down(KC_SHIFT_R),
        is_key_down(KC_OPTION_L) || is_key_down(KC_OPTION_R),
        is_key_down(KC_CMD_L) || is_key_down(KC_CMD_R),
    )
}

/// True if every required modifier in `need` is currently held.
fn required_mods_held(need: &ParsedHotkey, state: (bool, bool, bool, bool)) -> bool {
    let (ctrl, shift, option, cmd) = state;
    (!need.ctrl || ctrl)
        && (!need.shift || shift)
        && (!need.alt || option)
        && (!need.meta || cmd)
}

/// Map our hotkey key-name string to a Mac virtual key code. Hard-coded
/// for US QWERTY — covers >95% of keyboards. Phase 7 polish can add
/// TIS-based dynamic lookup for Dvorak/AZERTY layouts.
fn key_name_to_mac_keycode(name: &str) -> Option<u16> {
    Some(match name {
        "A" => 0, "B" => 11, "C" => 8, "D" => 2, "E" => 14, "F" => 3,
        "G" => 5, "H" => 4, "I" => 34, "J" => 38, "K" => 40, "L" => 37,
        "M" => 46, "N" => 45, "O" => 31, "P" => 35, "Q" => 12, "R" => 15,
        "S" => 1, "T" => 17, "U" => 32, "V" => 9, "W" => 13, "X" => 7,
        "Y" => 16, "Z" => 6,
        "0" => 29, "1" => 18, "2" => 19, "3" => 20, "4" => 21,
        "5" => 23, "6" => 22, "7" => 26, "8" => 28, "9" => 25,
        "Space" => 49, "Tab" => 48, "Return" | "Enter" => 36,
        "Backspace" => 51, "Escape" => 53,
        "F1" => 122, "F2" => 120, "F3" => 99, "F4" => 118, "F5" => 96,
        "F6" => 97, "F7" => 98, "F8" => 100, "F9" => 101, "F10" => 109,
        "F11" => 103, "F12" => 111,
        _ => return None,
    })
}

/// Holds the AtomicBool that keeps the current modifier-chord watcher
/// alive. Swapped on every re_register so only one watcher runs at a time.
fn modifier_chord_alive_slot() -> &'static Mutex<Option<Arc<AtomicBool>>> {
    static SLOT: OnceLock<Mutex<Option<Arc<AtomicBool>>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// Tear down anything platform-specific from a previous registration.
/// Called by [`super::re_register`] before installing fresh handlers.
pub fn stop_native_watchers() {
    let mut slot = modifier_chord_alive_slot().lock();
    if let Some(prev) = slot.take() {
        prev.store(false, Ordering::Relaxed);
    }
}

/// Long-running watcher for a modifier-only chord (e.g. ⌃⌘ held). Polls
/// modifier state every 25ms. Fires DictationPressed once all required
/// modifiers have been held for MODIFIER_CHORD_DEBOUNCE_MS, and
/// DictationReleased the instant any required modifier is released.
/// Stops when the AtomicBool stored in `modifier_chord_alive_slot()`
/// flips to false (next re_register swaps it).
pub fn spawn_modifier_chord_watcher(tx: Sender<HotkeyEvent>, hotkey: ParsedHotkey) {
    let alive = Arc::new(AtomicBool::new(true));
    *modifier_chord_alive_slot().lock() = Some(alive.clone());

    thread::spawn(move || {
        enum State {
            Idle,
            Arming(Instant),
            Pressing,
        }
        let mut state = State::Idle;
        let mut last_fire: Option<Instant> = None;
        tracing::info!("Mac modifier-chord watcher started for {:?}", hotkey);

        while alive.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(RELEASE_POLL_MS));
            let held = required_mods_held(&hotkey, modifier_state());
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
                        tracing::debug!("Mac modifier-chord dictation pressed: {:?}", hotkey);
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
        // Stopped mid-press → make sure release fires so the orchestrator
        // doesn't get stuck holding a recording open.
        if matches!(state, State::Pressing) {
            let _ = tx.send(HotkeyEvent::DictationReleased);
        }
        tracing::info!("Mac modifier-chord watcher exited for {:?}", hotkey);
    });
}

/// Hold-to-talk release detector for regular combos (e.g. ⌃⇧Space).
/// tauri-plugin-global-shortcut signals key-down via Carbon's
/// RegisterEventHotKey; we poll live key state to detect when the user
/// lets go. Fires `release_evt` once and exits.
pub fn spawn_release_poller(
    tx: Sender<HotkeyEvent>,
    hotkey: ParsedHotkey,
    release_evt: HotkeyEvent,
) {
    let Some(main_kc) = hotkey.key.as_deref().and_then(key_name_to_mac_keycode) else {
        tracing::warn!(
            "Mac release poller: no main key for hotkey {:?}; firing immediate release",
            hotkey
        );
        let _ = tx.send(release_evt);
        return;
    };

    thread::spawn(move || {
        // Safety net: never let this thread outlive 60s. If we somehow
        // miss the release edge, the orchestrator still gets a release.
        let started = Instant::now();
        loop {
            thread::sleep(Duration::from_millis(RELEASE_POLL_MS));
            if started.elapsed() > Duration::from_secs(RELEASE_POLL_TIMEOUT_SECS) {
                tracing::warn!("Mac release poller timed out — forcing release");
                let _ = tx.send(release_evt.clone());
                return;
            }
            let main_down = is_key_down(main_kc);
            let state = modifier_state();
            if !main_down || !required_mods_held(&hotkey, state) {
                let _ = tx.send(release_evt.clone());
                return;
            }
        }
    });
}
