// SPDX-License-Identifier: GPL-3.0-only
// Copyright (c) 2026 Romanch Roshan Singh

//! Direct evdev mouse-button reading for "Mouse mode" — a configured
//! button (default: middle-click) that toggles dictation on and off.
//!
//! Deliberately its own file rather than folded into `linux_evdev.rs`:
//! mouse buttons need a different device filter (pointing devices, not
//! keyboards) and none of the chord/hold matching that file's `Spec`
//! model exists for — a click is a single discrete down-edge, always
//! toggled via `super::route_mouse_click`, never held. Keeping this
//! separate means `linux_evdev.rs` — the keyboard hotkey's default Linux
//! path — is never touched by this feature.
//!
//! **Observe-only, same as `linux_evdev.rs`.** Reading /dev/input runs in
//! parallel with the compositor's own reading of the same device (via
//! libinput); it does not consume or block anything. So unlike Windows'
//! WH_MOUSE_LL (which sits inline in the delivery path and can genuinely
//! suppress a click), a mouse-mode click on Linux ALSO still does its
//! normal thing in the focused app — middle-click's X11 primary-paste,
//! or a browser's back/forward navigation for the side buttons — at the
//! same time Bulbul reacts to it. True suppression would need exclusive
//! device access (`EVIOCGRAB`) plus re-injecting every OTHER event
//! through a virtual `uinput` mouse — out of scope here; see the
//! conversation notes for why that's a bigger, riskier undertaking than
//! this pass takes on.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::thread;

use evdev::{Device, EventType, KeyCode};

use super::{HotkeyEvent, MouseButton};

static GENERATION: AtomicU64 = AtomicU64::new(0);

// Some mice report BTN_SIDE/BTN_EXTRA for the two side buttons, others
// report BTN_BACK/BTN_FORWARD for the same physical buttons — kernel/
// driver naming isn't consistent, so both codes are treated as the same
// logical button.
const C_MIDDLE: u16 = KeyCode::BTN_MIDDLE.0;
const C_SIDE: u16 = KeyCode::BTN_SIDE.0;
const C_EXTRA: u16 = KeyCode::BTN_EXTRA.0;
const C_BACK: u16 = KeyCode::BTN_BACK.0;
const C_FORWARD: u16 = KeyCode::BTN_FORWARD.0;

fn button_for_code(code: u16) -> Option<MouseButton> {
    match code {
        c if c == C_MIDDLE => Some(MouseButton::Middle),
        c if c == C_SIDE || c == C_BACK => Some(MouseButton::Back),
        c if c == C_EXTRA || c == C_FORWARD => Some(MouseButton::Forward),
        _ => None,
    }
}

fn is_mouse(d: &Device) -> bool {
    d.supported_keys()
        .is_some_and(|keys| keys.contains(KeyCode::BTN_LEFT) && keys.contains(KeyCode::BTN_MIDDLE))
}

fn mice() -> Vec<(std::path::PathBuf, Device)> {
    evdev::enumerate().filter(|(_, d)| is_mouse(d)).collect()
}

/// True when at least one mouse device is readable (same `input`-group
/// permission the keyboard evdev path needs).
pub fn available() -> bool {
    !mice().is_empty()
}

/// Stop all reader threads from the current registration.
pub fn stop() {
    GENERATION.fetch_add(1, Ordering::SeqCst);
}

/// Start watching every readable mouse for button-down edges. Replaces
/// any previous registration. Non-fatal if nothing's readable yet — the
/// caller already checked `available()`.
pub fn register(tx: Sender<HotkeyEvent>) {
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let devices = mice();
    if devices.is_empty() {
        return;
    }
    let count = devices.len();
    for (path, device) in devices {
        let tx = tx.clone();
        thread::Builder::new()
            .name("bulbul-evdev-mouse".into())
            .spawn(move || reader_loop(device, path, tx, generation))
            .ok();
    }
    tracing::info!("evdev mouse-mode watcher started on {count} device(s)");
}

fn reader_loop(mut device: Device, path: std::path::PathBuf, tx: Sender<HotkeyEvent>, generation: u64) {
    // Tracks which buttons are currently down, purely so we react to the
    // DOWN edge once (not on every event a held button might still emit)
    // — there's no "release" side to this at all, unlike the keyboard's
    // held-chord model.
    let mut held: HashSet<u16> = HashSet::new();
    loop {
        let events = match device.fetch_events() {
            Ok(e) => e,
            Err(e) => {
                tracing::debug!("evdev mouse reader for {path:?} ending: {e}");
                return;
            }
        };
        if GENERATION.load(Ordering::SeqCst) != generation {
            return;
        }
        for ev in events {
            if ev.event_type() != EventType::KEY {
                continue;
            }
            let code = ev.code();
            let was_held = held.contains(&code);
            match ev.value() {
                0 => {
                    held.remove(&code);
                }
                1 => {
                    held.insert(code);
                    if !was_held {
                        if let Some(btn) = button_for_code(code) {
                            if super::should_handle_mouse_click(btn) {
                                super::route_mouse_click(&tx);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
}
