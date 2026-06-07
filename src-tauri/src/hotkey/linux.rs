//! Linux-native hotkey primitives — Phase 0 stubs.
//!
//! Phase 4 replaces these with:
//! - X11: XGrabKey via `x11rb` + an `xcb_query_keymap`-based polling
//!   thread for release detection + modifier-only chord watching (the
//!   same shape as the Windows `GetAsyncKeyState` path).
//! - Wayland: ashpd calling `org.freedesktop.portal.GlobalShortcuts`.
//!   The portal delivers press AND release events natively, so no
//!   polling thread is needed on Wayland.
//!
//! Today (Phase 0): release polling fires immediately after a short
//! delay; modifier-chord is a hard no-op. The dev build boots and any
//! regular combo (e.g. `Ctrl+Shift+Space`) makes it as far as the press
//! handler — release fires too fast for real audio capture, which is
//! the explicit "Phase 4 needed" signal we want for the dev loop.

use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

use super::{HotkeyEvent, ParsedHotkey};

pub fn stop_native_watchers() {
    // No live watchers until Phase 4 wires up XGrabKey / portal listener.
}

pub fn spawn_modifier_chord_watcher(_tx: Sender<HotkeyEvent>, hotkey: ParsedHotkey) {
    tracing::warn!(
        "modifier-chord hotkeys not yet supported on Linux (Phase 4); \
         configure a regular combo (e.g. Ctrl+Shift+Space). Hotkey: {:?}",
        hotkey
    );
}

pub fn spawn_release_poller(
    tx: Sender<HotkeyEvent>,
    hotkey: ParsedHotkey,
    release_evt: HotkeyEvent,
) {
    tracing::warn!(
        "Linux release polling not yet implemented (Phase 4); firing immediate \
         release after stub delay. Hotkey: {:?}",
        hotkey
    );
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(200));
        let _ = tx.send(release_evt);
    });
}
