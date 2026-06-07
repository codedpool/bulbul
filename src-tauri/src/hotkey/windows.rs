//! Windows-native hotkey primitives: raw key-state polling for the
//! release detector and the modifier-only chord watcher.
//!
//! The cross-platform shell lives in `super` (mod.rs). This module
//! exposes only the three calls `super` makes into the native layer:
//! `stop_native_watchers`, `spawn_modifier_chord_watcher`, and
//! `spawn_release_poller`.

use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use super::{HotkeyEvent, ParsedHotkey, FIRE_COOLDOWN_MS};

/// How often the release poller checks the dictation hotkey's key state.
const RELEASE_POLL_MS: u64 = 25;
/// How long both modifiers of a modifier-only chord (e.g. Ctrl+Win) must
/// be held before we treat it as a dictation request. Short enough to feel
/// snappy, long enough that brushing both keys for unrelated reasons
/// (Ctrl+Win+arrow to switch desktop, etc.) doesn't fire dictation.
const MODIFIER_CHORD_DEBOUNCE_MS: u64 = 80;

/// Convert our key string to a Windows virtual-key code for `GetAsyncKeyState`.
fn key_name_to_vk(name: &str) -> Option<i32> {
    let code: i32 = match name {
        "Space" => 0x20,
        "Tab" => 0x09,
        "Return" | "Enter" => 0x0D,
        "Backspace" => 0x08,
        "Escape" => 0x1B,
        // Letters: VK_<A-Z> is just ASCII upper.
        x if x.len() == 1 && x.chars().next().unwrap().is_ascii_uppercase() => {
            x.chars().next().unwrap() as i32
        }
        // Digits: VK_0..VK_9 are ASCII '0'..'9'.
        x if x.len() == 1 && x.chars().next().unwrap().is_ascii_digit() => {
            x.chars().next().unwrap() as i32
        }
        // F1..F12 = 0x70..0x7B.
        x if x.starts_with('F') && x[1..].chars().all(|c| c.is_ascii_digit()) => {
            let n: u8 = x[1..].parse().ok()?;
            if (1..=12).contains(&n) {
                0x70 + (n as i32 - 1)
            } else {
                return None;
            }
        }
        _ => return None,
    };
    Some(code)
}

fn is_key_down(vk: i32) -> bool {
    use ::windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
    unsafe { GetAsyncKeyState(vk) < 0 }
}

/// Read the currently-held state of every modifier we care about.
/// Returns (ctrl, shift, alt, meta). VK_CONTROL/VK_SHIFT/VK_MENU are the
/// "either left or right" virtual keys; Win has separate L/R so we OR them.
fn modifier_state() -> (bool, bool, bool, bool) {
    use ::windows::Win32::UI::Input::KeyboardAndMouse::{
        VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
    };
    (
        is_key_down(VK_CONTROL.0 as i32),
        is_key_down(VK_SHIFT.0 as i32),
        is_key_down(VK_MENU.0 as i32),
        is_key_down(VK_LWIN.0 as i32) || is_key_down(VK_RWIN.0 as i32),
    )
}

/// True if every required modifier in `need` is currently held.
fn required_mods_held(need: &ParsedHotkey, state: (bool, bool, bool, bool)) -> bool {
    let (ctrl, shift, alt, meta) = state;
    (!need.ctrl || ctrl)
        && (!need.shift || shift)
        && (!need.alt || alt)
        && (!need.meta || meta)
}

/// Holds the AtomicBool that keeps the current modifier-chord watcher
/// alive. Swapped on every re_register so only one watcher runs at a time.
fn modifier_chord_alive_slot() -> &'static Mutex<Option<Arc<AtomicBool>>> {
    static SLOT: OnceLock<Mutex<Option<Arc<AtomicBool>>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// Tear down anything platform-specific from a previous registration.
/// Called by `super::re_register` before installing fresh handlers.
pub fn stop_native_watchers() {
    let mut slot = modifier_chord_alive_slot().lock();
    if let Some(prev) = slot.take() {
        prev.store(false, Ordering::Relaxed);
    }
}

/// Long-running watcher for a modifier-only chord like Ctrl+Win. Polls
/// modifier state every 25ms. Fires DictationPressed once both required
/// modifiers have been held for MODIFIER_CHORD_DEBOUNCE_MS, and
/// DictationReleased the instant either is released. Stops when the
/// AtomicBool stored in `modifier_chord_alive_slot()` flips to false
/// (next re_register swaps it).
pub fn spawn_modifier_chord_watcher(tx: Sender<HotkeyEvent>, hotkey: ParsedHotkey) {
    let alive = Arc::new(AtomicBool::new(true));
    *modifier_chord_alive_slot().lock() = Some(alive.clone());

    thread::spawn(move || {
        // State machine: Idle → Arming(start) → Pressing.
        // Cooldown across consecutive presses guards against accidental
        // re-trigger when the user briefly bounces a key.
        enum State {
            Idle,
            Arming(Instant),
            Pressing,
        }
        let mut state = State::Idle;
        let mut last_fire: Option<Instant> = None;
        tracing::info!("modifier-chord watcher started for {:?}", hotkey);
        while alive.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(RELEASE_POLL_MS));
            let held = required_mods_held(&hotkey, modifier_state());
            match state {
                State::Idle => {
                    if held {
                        let cooled = last_fire.map_or(true, |t| {
                            t.elapsed().as_millis() >= FIRE_COOLDOWN_MS
                        });
                        if cooled {
                            state = State::Arming(Instant::now());
                        }
                    }
                }
                State::Arming(start) => {
                    if !held {
                        // Released before the debounce — treat as noise.
                        state = State::Idle;
                    } else if start.elapsed().as_millis()
                        >= MODIFIER_CHORD_DEBOUNCE_MS as u128
                    {
                        tracing::debug!("modifier-chord dictation pressed: {:?}", hotkey);
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
        // Stopped — if we were mid-press make sure release fires so the
        // orchestrator doesn't get stuck holding a recording open.
        if matches!(state, State::Pressing) {
            let _ = tx.send(HotkeyEvent::DictationReleased);
        }
        tracing::info!("modifier-chord watcher exited for {:?}", hotkey);
    });
}

/// Hold-to-talk release detector. RegisterHotKey only signals key-down;
/// we poll the actual key state to detect when the user lets go. The
/// `release_evt` parameter lets the same poller drive either the
/// dictation pipeline or the voice-transform pipeline.
pub fn spawn_release_poller(
    tx: Sender<HotkeyEvent>,
    hotkey: ParsedHotkey,
    release_evt: HotkeyEvent,
) {
    use ::windows::Win32::UI::Input::KeyboardAndMouse::{
        VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
    };
    let Some(main_vk) = hotkey.key.as_deref().and_then(key_name_to_vk) else {
        return;
    };
    thread::spawn(move || {
        // Safety net: regardless of polling result, never let this thread
        // outlive a reasonable upper bound on a press. 60 seconds covers
        // even the most generous dictation; if we hit it we fire release
        // anyway so the orchestrator doesn't get stuck.
        let started = Instant::now();
        loop {
            thread::sleep(Duration::from_millis(RELEASE_POLL_MS));
            if started.elapsed() > Duration::from_secs(60) {
                tracing::warn!("release poller timed out — forcing release");
                let _ = tx.send(release_evt.clone());
                return;
            }
            let main_down = is_key_down(main_vk);
            let ctrl_ok = !hotkey.ctrl || is_key_down(VK_CONTROL.0 as i32);
            let shift_ok = !hotkey.shift || is_key_down(VK_SHIFT.0 as i32);
            let alt_ok = !hotkey.alt || is_key_down(VK_MENU.0 as i32);
            let meta_ok = !hotkey.meta
                || is_key_down(VK_LWIN.0 as i32)
                || is_key_down(VK_RWIN.0 as i32);
            if !(main_down && ctrl_ok && shift_ok && alt_ok && meta_ok) {
                let _ = tx.send(release_evt.clone());
                return;
            }
        }
    });
}
