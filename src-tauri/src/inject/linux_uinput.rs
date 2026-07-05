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
//! default. The .deb grants it narrowly: a dedicated `bulbul-input`
//! system group owns the device (via a udev rule) and the installed
//! binary is setgid to that group — so Bulbul runs able to open uinput
//! and nothing else, with no relogin and no broad capability. See
//! `deb/postinst.sh`.
//!
//! We keep one virtual device alive for the process lifetime (a uinput
//! device vanishes when its handle drops) and only ever send the paste/
//! copy chord — never free-form typed text, which would need per-layout
//! keycode mapping. Clipboard + Ctrl+V stays robust across layouts.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use evdev::{uinput::VirtualDevice, AttributeSet, EventType, InputEvent, KeyCode};
use parking_lot::Mutex;

pub const KEY_V: KeyCode = KeyCode::KEY_V;
pub const KEY_C: KeyCode = KeyCode::KEY_C;
const KEY_LEFTCTRL: KeyCode = KeyCode::KEY_LEFTCTRL;

const PRESS: i32 = 1;
const RELEASE: i32 = 0;
// Small gap between synthetic events. Some apps debounce or miss a chord
// whose press/release land in the same kernel tick; a couple ms each
// makes the combo read as deliberate without adding perceptible latency.
const EVENT_GAP: Duration = Duration::from_millis(4);

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
    keys.insert(KEY_LEFTCTRL);
    keys.insert(KEY_V);
    keys.insert(KEY_C);

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

    let emit = |device: &mut VirtualDevice, key: KeyCode, val: i32| -> Result<()> {
        // evdev 0.13 has no KeyEvent helper — build the raw InputEvent
        // (KEY event type, the key's scancode, press/release value).
        let ev = InputEvent::new(EventType::KEY.0, key.code(), val);
        device
            .emit(&[ev])
            .with_context(|| format!("emitting {key:?}={val}"))?;
        Ok(())
    };

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
