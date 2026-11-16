// SPDX-License-Identifier: GPL-3.0-only
// Copyright (c) 2026 Romanch Roshan Singh

//! Direct evdev mouse-button reading for "Mouse mode" — a configured
//! button (default: middle-click) that toggles dictation on and off.
//!
//! Deliberately its own file rather than folded into `linux_evdev.rs`:
//! mouse buttons need a different device filter and none of the
//! chord/hold matching that file's `Spec` model exists for — a click is
//! a single discrete down-edge, always toggled via
//! `super::route_mouse_click`, never held.
//!
//! **Suppresses the configured button, same as Windows,** via `EVIOCGRAB`
//! exclusive access plus a virtual mouse (`uinput`) that mirrors the real
//! one and re-emits every event except the swallowed button's down/up
//! pair. A grab takes the device away from the compositor entirely, so
//! the whole design hinges on never leaving a device grabbed with nothing
//! forwarding its events — a dead pointer needs a reboot to explain.
//! The rules that keep that from happening:
//!
//! - Only plain relative mice are ever grabbed (`mouse_kind`). Touchpads
//!   (absolute axes) and pointing sticks are only watched, never grabbed,
//!   and Bulbul's own virtual mouse is ignored — a mirror that only
//!   copies buttons and relative axes would leave those with no working
//!   pointer.
//! - Nothing is grabbed at all unless Mouse mode is on.
//! - The virtual mouse is created first and given `GRAB_SETTLE` to be
//!   adopted by udev/the compositor *before* the real device is taken.
//! - Readers poll with a timeout, so a stop/re-register is honoured
//!   within `POLL_MS` instead of waiting for the next mouse event, and the
//!   device is ungrabbed before its virtual mirror is destroyed.
//! - If forwarding ever fails, the device is released at once.
//! - Re-registering while a registration is already live is a no-op, so
//!   hotkey/transform edits don't churn grabs.
//!
//! If the mirror can't be built or the grab fails, it falls back to
//! observing clicks without suppressing them. On an X11 session
//! `linux_xgrab.rs` additionally keeps the button from other apps whatever
//! the pointer type, which covers exactly those fallback cases.

use std::collections::HashSet;
use std::os::fd::{AsRawFd, RawFd};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use evdev::uinput::{VirtualDevice, VirtualDeviceBuilder};
use evdev::{Device, EventType, InputEvent, KeyCode, PropType, RelativeAxisCode};

use super::{HotkeyEvent, MouseButton};

const VIRTUAL_NAME: &str = "Bulbul Virtual Mouse";

// Grace period before the FIRST registration each process lifetime does
// anything: it fires from setup() on every launch, including an
// autostart-at-login one, while the session is still settling.
const FIRST_GRAB_GRACE: Duration = Duration::from_secs(3);
// Time given to udev + the compositor to adopt the virtual mouse before
// the real one is taken away.
const GRAB_SETTLE: Duration = Duration::from_millis(400);
// Longest a reader blocks before re-checking whether it was superseded.
const POLL_MS: i32 = 100;
// Don't grab mid-click: a press the virtual mouse never saw would leave
// the button stuck down for the compositor.
const BUTTON_RELEASE_WAIT: Duration = Duration::from_secs(2);

/// Bumped by every `stop()`/`start()`; a reader whose generation no
/// longer matches exits (ungrabbing first).
static GENERATION: AtomicU64 = AtomicU64::new(0);
/// Generation of the registration that is currently meant to be running.
static STARTED_GEN: AtomicU64 = AtomicU64::new(0);
/// Reader threads (including ones still setting up) not yet exited.
static LIVE: AtomicUsize = AtomicUsize::new(0);
static FIRST_REGISTER: AtomicBool = AtomicBool::new(true);
static GRACE_PENDING: AtomicBool = AtomicBool::new(false);
static TX: Mutex<Option<Sender<HotkeyEvent>>> = Mutex::new(None);
static SYNC_LOCK: Mutex<()> = Mutex::new(());

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

/// How a pointing device should be treated.
enum MouseKind {
    /// Real, plain relative mouse — the only kind a button-and-relative-
    /// axis mirror can stand in for without losing anything.
    Grabbable,
    /// Has a middle button but isn't safe to mirror (touchpad, tablet-style
    /// pointer in a VM, pointing stick): clicks are watched, never grabbed.
    ObserveOnly,
}

fn mouse_kind(d: &Device) -> Option<MouseKind> {
    if d.name() == Some(VIRTUAL_NAME) {
        return None;
    }
    let has_buttons = d
        .supported_keys()
        .is_some_and(|k| k.contains(KeyCode::BTN_LEFT) && k.contains(KeyCode::BTN_MIDDLE));
    if !has_buttons {
        return None;
    }
    let has_rel_xy = d.supported_relative_axes().is_some_and(|a| {
        a.contains(RelativeAxisCode::REL_X) && a.contains(RelativeAxisCode::REL_Y)
    });
    let no_abs = d.supported_absolute_axes().is_none();
    let not_stick = !d.properties().contains(PropType::POINTING_STICK);
    if has_rel_xy && no_abs && not_stick {
        Some(MouseKind::Grabbable)
    } else {
        Some(MouseKind::ObserveOnly)
    }
}

fn mice() -> Vec<(PathBuf, Device, MouseKind)> {
    evdev::enumerate()
        .filter_map(|(path, d)| mouse_kind(&d).map(|k| (path, d, k)))
        .collect()
}

fn is_current(generation: u64) -> bool {
    GENERATION.load(Ordering::SeqCst) == generation
}

/// Stop all reader threads. Each one ungrabs its device and exits within
/// `POLL_MS`.
pub fn stop() {
    GENERATION.fetch_add(1, Ordering::SeqCst);
}

/// Called whenever hotkeys are (re)registered. Remembers the event sink
/// and brings the mouse watcher in line with the current Mouse mode
/// setting. The very first call each process lifetime is deferred by
/// `FIRST_GRAB_GRACE`.
pub fn register(tx: Sender<HotkeyEvent>) {
    *TX.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);
    if FIRST_REGISTER.swap(false, Ordering::SeqCst) {
        GRACE_PENDING.store(true, Ordering::SeqCst);
        let spawned = thread::Builder::new()
            .name("bulbul-mouse-mode-grace".into())
            .spawn(|| {
                thread::sleep(FIRST_GRAB_GRACE);
                GRACE_PENDING.store(false, Ordering::SeqCst);
                sync();
            });
        if spawned.is_err() {
            GRACE_PENDING.store(false, Ordering::SeqCst);
            sync();
        }
        return;
    }
    sync();
}

/// Starts or stops the watcher to match the Mouse mode setting. Safe to
/// call repeatedly: a registration that is already live is left alone.
pub fn sync() {
    let _guard = SYNC_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if GRACE_PENDING.load(Ordering::SeqCst) {
        return;
    }
    let Some(tx) = TX.lock().unwrap_or_else(|e| e.into_inner()).clone() else {
        return;
    };
    if !super::mouse_mode_enabled() {
        stop();
        return;
    }
    if STARTED_GEN.load(Ordering::SeqCst) == GENERATION.load(Ordering::SeqCst)
        && LIVE.load(Ordering::SeqCst) > 0
    {
        return;
    }
    start(tx);
}

fn start(tx: Sender<HotkeyEvent>) {
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    STARTED_GEN.store(generation, Ordering::SeqCst);
    let mice = mice();
    let count = mice.len();
    for (path, mut device, kind) in mice {
        LIVE.fetch_add(1, Ordering::SeqCst);
        let tx = tx.clone();
        let spawned = thread::Builder::new()
            .name("bulbul-evdev-mouse".into())
            .spawn(move || {
                let _live = LiveGuard;
                match kind {
                    MouseKind::Grabbable => run_device(device, path, tx, generation),
                    MouseKind::ObserveOnly => {
                        observe_only_loop(&mut device, &path, &tx, generation)
                    }
                }
            });
        if spawned.is_err() {
            LIVE.fetch_sub(1, Ordering::SeqCst);
        }
    }
    tracing::info!("evdev mouse-mode watcher starting on {count} pointing device(s)");

    // Pointers that can't be taken over (touchpads, a VM's absolute pointer)
    // only get watched above, so the desktop would still act on the click —
    // on Linux a middle click pastes the selection. On X11, also ask the
    // display to keep the button from other apps. It follows the Settings
    // value and ends with this registration.
    super::linux_xgrab::spawn(move || is_current(generation), x_button_wanted);
}

/// The X button number Mouse mode wants kept from other apps right now, or
/// `None` while the mode is off.
fn x_button_wanted() -> Option<u8> {
    if !super::mouse_mode_enabled() {
        return None;
    }
    let btn = MouseButton::from_u8(super::MOUSE_BUTTON.load(Ordering::SeqCst));
    Some(match btn {
        MouseButton::Middle => 2,
        MouseButton::Back => 8,
        MouseButton::Forward => 9,
    })
}

struct LiveGuard;
impl Drop for LiveGuard {
    fn drop(&mut self) {
        LIVE.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Builds a virtual mouse mirroring `device`'s buttons, relative axes and
/// MSC scan-code passthrough, so re-emitted events read as a normal,
/// fully-functional mouse downstream.
fn build_virtual_mirror(device: &Device) -> std::io::Result<VirtualDevice> {
    let mut builder = VirtualDeviceBuilder::new()?.name(VIRTUAL_NAME);
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

fn run_device(mut device: Device, path: PathBuf, tx: Sender<HotkeyEvent>, generation: u64) {
    match build_virtual_mirror(&device) {
        Ok(virt) => {
            if !settle(GRAB_SETTLE, generation) {
                return;
            }
            wait_buttons_released(&device, generation);
            if !is_current(generation) {
                return;
            }
            if grab_with_retry(&mut device, generation) {
                tracing::info!("mouse mode: suppressing on {path:?}");
                grabbed_loop(&mut device, virt, &path, &tx, generation);
                return;
            }
            tracing::warn!(
                "mouse mode: couldn't grab {path:?} (already grabbed by something else?) — \
                 reacting to clicks but not suppressing on this device"
            );
            drop(virt);
        }
        Err(e) => {
            tracing::warn!(
                "mouse mode: couldn't build a virtual mouse for {path:?}: {e:#} — \
                 reacting to clicks but not suppressing on this device"
            );
        }
    }
    if is_current(generation) {
        observe_only_loop(&mut device, &path, &tx, generation);
    }
}

/// Sleeps up to `total` in short slices; false if superseded meanwhile.
fn settle(total: Duration, generation: u64) -> bool {
    let deadline = Instant::now() + total;
    while Instant::now() < deadline {
        if !is_current(generation) {
            return false;
        }
        thread::sleep(Duration::from_millis(50));
    }
    is_current(generation)
}

fn wait_buttons_released(device: &Device, generation: u64) {
    let deadline = Instant::now() + BUTTON_RELEASE_WAIT;
    while Instant::now() < deadline && is_current(generation) {
        match device.get_key_state() {
            Ok(keys)
                if !keys.contains(KeyCode::BTN_LEFT)
                    && !keys.contains(KeyCode::BTN_RIGHT)
                    && !keys.contains(KeyCode::BTN_MIDDLE) =>
            {
                return;
            }
            Ok(_) => {}
            Err(_) => return,
        }
        thread::sleep(Duration::from_millis(30));
    }
}

/// A previous registration's reader may still be releasing the same
/// device (EBUSY) for a moment after `stop()`.
fn grab_with_retry(device: &mut Device, generation: u64) -> bool {
    for _ in 0..10 {
        if !is_current(generation) {
            return false;
        }
        match device.grab() {
            Ok(()) => return true,
            Err(e) if e.raw_os_error() == Some(libc::EBUSY) => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                tracing::debug!("mouse mode: grab failed: {e}");
                return false;
            }
        }
    }
    false
}

/// Waits until `fd` is readable. Ok(false) on timeout/interrupt, Err when
/// the device has gone away.
fn wait_readable(fd: RawFd, timeout_ms: i32) -> std::io::Result<bool> {
    let mut p = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
    let r = unsafe { libc::poll(&mut p, 1, timeout_ms) };
    if r < 0 {
        let e = std::io::Error::last_os_error();
        if e.kind() == std::io::ErrorKind::Interrupted {
            return Ok(false);
        }
        return Err(e);
    }
    if p.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
        return Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe));
    }
    Ok(r > 0 && p.revents & libc::POLLIN != 0)
}

/// Reads one batch into an owned Vec so `device`'s borrow ends when this
/// returns (`FetchEventsSynced` has a Drop impl that otherwise keeps the
/// device borrowed in the caller).
fn fetch_batch(device: &mut Device) -> std::io::Result<Vec<InputEvent>> {
    Ok(device.fetch_events()?.collect())
}

/// Forwarder for an exclusively grabbed device. Re-frames events exactly
/// as the kernel framed them: accumulates everything between SYN_REPORTs
/// and re-emits that batch (VirtualDevice::emit appends its own
/// SYN_REPORT) — except the configured button's down/up pair, which is
/// dropped and drives `route_mouse_click` instead. Tracks the specific raw
/// code it swallowed the down-edge for, so a config change mid-click can't
/// leave a stray unmatched release forwarded or a real one eaten.
///
/// Always releases the real device before dropping the virtual one.
fn grabbed_loop(
    device: &mut Device,
    mut virt: VirtualDevice,
    path: &Path,
    tx: &Sender<HotkeyEvent>,
    generation: u64,
) {
    let fd = device.as_raw_fd();
    let mut batch: Vec<InputEvent> = Vec::new();
    let mut swallowed_code: Option<u16> = None;
    'outer: while is_current(generation) {
        match wait_readable(fd, POLL_MS) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(e) => {
                tracing::debug!("evdev mouse (grabbed) reader for {path:?} ending: {e}");
                break;
            }
        }
        let events = match fetch_batch(device) {
            Ok(events) => events,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(e) => {
                tracing::debug!("evdev mouse (grabbed) reader for {path:?} ending: {e}");
                break;
            }
        };
        for ev in events {
            if ev.event_type() == EventType::SYNCHRONIZATION {
                if !batch.is_empty() {
                    if let Err(e) = virt.emit(&batch) {
                        tracing::warn!(
                            "mouse mode: virtual mouse emit failed for {path:?}: {e:#} — \
                             releasing the device"
                        );
                        break 'outer;
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
                            super::route_mouse_click(tx);
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
    let _ = device.ungrab();
    drop(virt);
}

/// Fallback when grabbing or mirroring failed: reacts to the configured
/// button but can't suppress it, since nothing else is forwarding this
/// device's other events for us.
fn observe_only_loop(
    device: &mut Device,
    path: &Path,
    tx: &Sender<HotkeyEvent>,
    generation: u64,
) {
    let fd = device.as_raw_fd();
    let mut held: HashSet<u16> = HashSet::new();
    while is_current(generation) {
        match wait_readable(fd, POLL_MS) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(e) => {
                tracing::debug!("evdev mouse (observe-only) reader for {path:?} ending: {e}");
                return;
            }
        }
        let events = match fetch_batch(device) {
            Ok(events) => events,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(e) => {
                tracing::debug!("evdev mouse (observe-only) reader for {path:?} ending: {e}");
                return;
            }
        };
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
                                super::route_mouse_click(tx);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
}
