// SPDX-License-Identifier: GPL-3.0-only
// Copyright (c) 2026 Romanch Roshan Singh

//! Direct evdev mouse-button reading for "Mouse mode" — a configured
//! button (default: middle-click) that toggles dictation on and off.
//!
//! Deliberately its own file rather than folded into `linux_evdev.rs`:
//! mouse buttons need a different device filter and none of the
//! chord/hold matching that file's `Spec` model exists for — a click is
//! a single discrete down-edge, always toggled via
//! `super::route_mouse_click`, never held. `linux_evdev.rs` — the
//! keyboard hotkey's default Linux path — is never touched by this file.
//!
//! **Genuinely suppresses the configured button, same as Windows,** via
//! `EVIOCGRAB` (`Device::grab()`) exclusive access plus a virtual mouse
//! (`evdev::uinput::VirtualDeviceBuilder`) that mirrors the real one's
//! capabilities and re-emits every event except the swallowed button's
//! down/up pair. Grabbing takes the device away from the compositor
//! entirely, so without the passthrough the cursor would freeze —
//! that's the whole reason the virtual mirror exists.
//!
//! Falls back to observing without suppressing (matching the keyboard
//! evdev path's existing behavior) if building the virtual mirror or
//! grabbing the device fails for any reason — never leaves a device
//! grabbed with nothing forwarding its events. Both the `evdev` crate's
//! grab/uinput APIs are already a dependency (used elsewhere in this
//! module already); no new crate was added for this.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::thread;

use evdev::uinput::{VirtualDevice, VirtualDeviceBuilder};
use evdev::{Device, EventType, InputEvent, KeyCode};

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

fn mice() -> Vec<(PathBuf, Device)> {
    evdev::enumerate().filter(|(_, d)| is_mouse(d)).collect()
}

/// True when at least one mouse device is readable (same `input`-group
/// permission the keyboard evdev path needs).
pub fn available() -> bool {
    !mice().is_empty()
}

/// Stop all reader threads from the current registration. Each thread
/// ungrabs its device (if grabbed) before exiting.
pub fn stop() {
    GENERATION.fetch_add(1, Ordering::SeqCst);
}

/// Builds a virtual mouse mirroring `device`'s own capabilities (buttons,
/// relative axes, MSC scan-code passthrough), so re-emitted events read
/// as a normal, fully-functional mouse to everything downstream. Only
/// mirrors what the real device actually reports — an unusual mouse
/// missing an axis type just means the virtual one skips it too.
fn build_virtual_mirror(device: &Device) -> std::io::Result<VirtualDevice> {
    let mut builder = VirtualDeviceBuilder::new()?.name("Bulbul Virtual Mouse");
    if let Some(keys) = device.supported_keys() {
        builder = builder.with_keys(keys)?;
    }
    if let Some(axes) = device.supported_relative_axes() {
        builder = builder.with_relative_axes(axes)?;
    }
    if let Some(misc) = device.misc_properties() {
        builder = builder.with_msc(misc)?;
    }
    builder.build()
}

/// Start watching every readable mouse. Replaces any previous
/// registration. Non-fatal if nothing's readable yet — the caller
/// already checked `available()`.
pub fn register(tx: Sender<HotkeyEvent>) {
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let devices = mice();
    if devices.is_empty() {
        return;
    }
    let count = devices.len();
    let mut suppressing = 0u32;
    for (path, mut device) in devices {
        let mirrored = build_virtual_mirror(&device);
        let grabbed = match &mirrored {
            Ok(_) => device.grab().is_ok(),
            Err(_) => false,
        };
        match (mirrored, grabbed) {
            (Ok(virt), true) => {
                suppressing += 1;
                let tx = tx.clone();
                thread::Builder::new()
                    .name("bulbul-evdev-mouse".into())
                    .spawn(move || grabbed_reader_loop(device, virt, path, tx, generation))
                    .ok();
            }
            (mirror_result, _) => {
                if let Err(e) = mirror_result {
                    tracing::warn!(
                        "mouse mode: couldn't build a virtual mouse for {path:?}: {e:#} — \
                         reacting to clicks but not suppressing on this device"
                    );
                } else {
                    tracing::warn!(
                        "mouse mode: couldn't grab {path:?} (already grabbed by something else?) — \
                         reacting to clicks but not suppressing on this device"
                    );
                }
                let tx = tx.clone();
                thread::Builder::new()
                    .name("bulbul-evdev-mouse".into())
                    .spawn(move || observe_only_reader_loop(device, path, tx, generation))
                    .ok();
            }
        }
    }
    tracing::info!(
        "evdev mouse-mode watcher started on {count} device(s), {suppressing} with true suppression"
    );
}

/// Reader loop for a device we've exclusively grabbed. Re-frames events
/// exactly as the kernel framed them: accumulates everything between
/// SYN_REPORTs and re-emits that batch (VirtualDevice::emit appends its
/// own SYN_REPORT) — except the configured button's down/up pair, which
/// is dropped from the batch entirely and drives `route_mouse_click`
/// instead. Tracks the specific raw code it swallowed the down-edge for,
/// not "whatever's currently configured", so a config change mid-click
/// can't leave a stray unmatched release forwarded or a real one eaten.
fn grabbed_reader_loop(
    mut device: Device,
    mut virt: VirtualDevice,
    path: PathBuf,
    tx: Sender<HotkeyEvent>,
    generation: u64,
) {
    let mut batch: Vec<InputEvent> = Vec::new();
    let mut swallowed_code: Option<u16> = None;
    loop {
        let events = match device.fetch_events() {
            Ok(e) => e,
            Err(e) => {
                tracing::debug!("evdev mouse (grabbed) reader for {path:?} ending: {e}");
                let _ = device.ungrab();
                return;
            }
        };
        if GENERATION.load(Ordering::SeqCst) != generation {
            let _ = device.ungrab();
            return;
        }
        for ev in events {
            if ev.event_type() == EventType::SYNCHRONIZATION {
                if !batch.is_empty() {
                    if let Err(e) = virt.emit(&batch) {
                        tracing::warn!("mouse mode: virtual mouse emit failed for {path:?}: {e:#}");
                    }
                    batch.clear();
                }
                continue;
            }
            if ev.event_type() == EventType::KEY {
                let code = ev.code();
                if let Some(btn) = button_for_code(code) {
                    match ev.value() {
                        1 if super::should_handle_mouse_click(btn) => {
                            swallowed_code = Some(code);
                            super::route_mouse_click(&tx);
                            continue; // drop the down edge — don't forward
                        }
                        0 if swallowed_code == Some(code) => {
                            swallowed_code = None;
                            continue; // drop the matching up edge too
                        }
                        _ => {}
                    }
                }
            }
            batch.push(ev);
        }
    }
}

/// Fallback path when grabbing or mirroring failed: identical to the
/// original observe-only watcher — reacts to the configured button but
/// can't suppress it, since nothing else is forwarding this device's
/// other events for us.
fn observe_only_reader_loop(
    mut device: Device,
    path: PathBuf,
    tx: Sender<HotkeyEvent>,
    generation: u64,
) {
    let mut held: HashSet<u16> = HashSet::new();
    loop {
        let events = match device.fetch_events() {
            Ok(e) => e,
            Err(e) => {
                tracing::debug!("evdev mouse (observe-only) reader for {path:?} ending: {e}");
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
