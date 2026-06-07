//! macOS-native hotkey primitives — Phase 0 stubs.
//!
//! Phase 4 replaces these with CGEventTap. Spec:
//! - `spawn_release_poller`: a tap watching `keyDown`/`keyUp`/`flagsChanged`
//!   on `.cgSessionEventTap` that converts the key-up edge into a
//!   release event.
//! - `spawn_modifier_chord_watcher`: same tap, but matching modifier-flag
//!   transitions instead of a non-modifier key.
//! - `stop_native_watchers`: invalidate the tap's CFRunLoopSource.
//!
//! Today (Phase 0): release polling fires a release event after a short
//! delay so the orchestrator doesn't hang in "recording" state forever
//! on a Mac dev build. Audio captured in that window will be too short
//! for Groq to transcribe — the user sees a "no speech detected" error,
//! which is the explicit "this needs Phase 4" signal we want for the
//! tester loop. Modifier-chord watcher is a hard no-op: users should
//! configure a regular Ctrl+Shift+Space-style combo in dev for now.

use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

use super::{HotkeyEvent, ParsedHotkey};

pub fn stop_native_watchers() {
    // No live watchers to stop until Phase 4 lands CGEventTap.
}

pub fn spawn_modifier_chord_watcher(_tx: Sender<HotkeyEvent>, hotkey: ParsedHotkey) {
    tracing::warn!(
        "modifier-chord hotkeys not yet supported on macOS (Phase 4); \
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
        "macOS release polling not yet implemented (Phase 4); firing immediate release \
         after stub delay. Hotkey: {:?}",
        hotkey
    );
    // Give the press event a beat to flow through the orchestrator before
    // we immediately follow up with release — otherwise the orchestrator
    // can receive press+release before its recording thread is ready.
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(200));
        let _ = tx.send(release_evt);
    });
}
