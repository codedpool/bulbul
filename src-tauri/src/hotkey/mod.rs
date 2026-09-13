// SPDX-License-Identifier: GPL-3.0-only
// Copyright (c) 2026 Romanch Roshan Singh

//! Hotkey registration + the orchestrator-bound event channel.
//!
//! This module owns the platform-agnostic parts:
//! - the parsed-hotkey/event/status types,
//! - key-name → W3C `Code` mapping (the cross-platform global-shortcut
//!   plugin speaks Codes),
//! - the dictation / polish / transform-slot registration logic.
//!
//! Anything that has to touch raw OS APIs (querying live key state for a
//! release poller, or polling modifier state for a modifier-only chord)
//! lives in `windows.rs` / `macos.rs`. Those modules expose a small
//! native-side surface (`stop_native_watchers`, `spawn_modifier_chord_watcher`,
//! `spawn_release_poller`) which this module calls without caring about
//! the underlying primitives.

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use tauri::AppHandle;
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
use windows as native;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as native;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as native;
#[cfg(target_os = "linux")]
mod linux_portal;
#[cfg(target_os = "linux")]
mod linux_evdev;
#[cfg(target_os = "linux")]
mod linux_mouse;

/// Minimum gap between two fires of the same hotkey. Guards against
/// auto-repeat and spurious event bursts. Used by mod.rs's per-shortcut
/// handlers; the native release-poller has its own cadence constants.
pub const FIRE_COOLDOWN_MS: u128 = 700;

#[derive(Clone, Debug)]
pub enum HotkeyEvent {
    DictationPressed,
    DictationReleased,
    PolishDictationPressed,
    PolishDictationReleased,
    TransformTriggered(i64),
}

// ─── "Tap to talk" ──────────────────────────────────────────────────────
//
// When on, the dictation/polish hotkeys toggle instead of requiring a
// hold: one tap starts, the next tap stops. A plain global rather than
// something threaded through HotkeySet/re_register, because the Windows
// LL keyboard hook (keyboard_hook.rs) is installed ONCE at boot and isn't
// re-created per hotkey registration the way the other physical-key
// producers are — a global is the one thing every producer can reach
// without re-plumbing each of their call sites individually.
//
// `route_physical_event` is the single choke point every PHYSICAL
// press/release producer sends through instead of hitting `tx` directly:
// the Windows LL keyboard hook (modifier-only chords like the default
// Ctrl+Win), the global-shortcut handler + native release poller (regular
// combos, e.g. Shift+Alt+P), and Linux evdev (direct /dev/input reading,
// the default Linux path once the user has input-device access).
//
// Deliberately NOT wired into two other producers:
//   - `cli_toggle_dictation` (desktop.rs) — the Linux CLI/signal escape
//     hatch for GNOME Wayland users whose compositor can't register the
//     hotkey at all. It already has its own complete, independent toggle
//     state and sends straight to AppState.hotkey_tx without going
//     through hotkey::re_register or this function at all, so it's
//     structurally unaffected by this setting either way — which is
//     exactly what keeps it working regardless of whether "Tap to talk"
//     is on.
//   - `linux_portal.rs` (the Wayland GlobalShortcuts portal, used when
//     evdev access isn't available yet) — it already implements its own
//     toggle-tolerance as a workaround for a GNOME bug where a held
//     shortcut sometimes never emits a release signal at all: a second
//     Activated while already active is treated as the missing release
//     and turned into a synthesized `DictationReleased`. Passing that
//     synthesized release back through this translation would swallow it
//     (tap mode treats a raw release as "ignore, wait for the next tap"),
//     silently breaking the exact GNOME workaround it depends on. Until
//     that's unified deliberately, "Tap to talk" simply has no effect on
//     the portal path — a real hold still behaves as a real hold there.
static TAP_TO_TALK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static DICTATION_TAP_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static POLISH_TAP_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Updates the shared "Tap to talk" flag every physical producer reads.
/// Call at boot (from the loaded config) and whenever Settings saves a
/// change to it. Resets both per-hotkey toggle states so flipping the
/// setting can never leave a stale "already active" flag around that
/// would silently eat the next real tap.
pub fn set_tap_to_talk_enabled(on: bool) {
    use std::sync::atomic::Ordering;
    TAP_TO_TALK.store(on, Ordering::SeqCst);
    DICTATION_TAP_ACTIVE.store(false, Ordering::SeqCst);
    POLISH_TAP_ACTIVE.store(false, Ordering::SeqCst);
}

/// Routes one raw physical press/release through "Tap to talk"
/// translation when it's on, before forwarding to `tx`: a tap starts
/// (forwards Pressed), a raw release is swallowed, and the next tap stops
/// (forwards Released instead of Pressed). Passed straight through
/// unchanged when the setting is off, and for event kinds it doesn't
/// apply to (`TransformTriggered` — slots are already tap-to-trigger).
pub fn route_physical_event(evt: HotkeyEvent, tx: &Sender<HotkeyEvent>) {
    use std::sync::atomic::Ordering;
    if !TAP_TO_TALK.load(Ordering::SeqCst) {
        let _ = tx.send(evt);
        return;
    }
    match evt {
        HotkeyEvent::DictationPressed => {
            toggle_forward(&DICTATION_TAP_ACTIVE, HotkeyEvent::DictationPressed, HotkeyEvent::DictationReleased, tx)
        }
        HotkeyEvent::DictationReleased => {}
        HotkeyEvent::PolishDictationPressed => toggle_forward(
            &POLISH_TAP_ACTIVE,
            HotkeyEvent::PolishDictationPressed,
            HotkeyEvent::PolishDictationReleased,
            tx,
        ),
        HotkeyEvent::PolishDictationReleased => {}
        other => {
            let _ = tx.send(other);
        }
    }
}

fn toggle_forward(
    active: &std::sync::atomic::AtomicBool,
    pressed: HotkeyEvent,
    released: HotkeyEvent,
    tx: &Sender<HotkeyEvent>,
) {
    use std::sync::atomic::Ordering;
    if !active.swap(true, Ordering::SeqCst) {
        let _ = tx.send(pressed);
    } else {
        active.store(false, Ordering::SeqCst);
        let _ = tx.send(released);
    }
}

// ─── "Mouse mode" ───────────────────────────────────────────────────────
//
// A user-configured mouse button (default: middle-click) always toggles
// dictation — click to start, click again to stop — independent of
// "Tap to talk" above, which only governs the keyboard hotkey. This is
// the user's separate choice to dictate via a click at all.
//
// All three platforms genuinely suppress the configured button's normal
// effect (browser back/forward, X11 primary-paste, etc.) while Mouse
// mode is on — not just react to it:
//   - Windows: `mouse_hook.rs`'s WH_MOUSE_LL hook sits inline in the
//     delivery path.
//   - Linux: `linux_mouse.rs` exclusively grabs the mouse device
//     (EVIOCGRAB) and re-emits everything except the configured button
//     through a virtual mouse (evdev::uinput) that mirrors the real
//     one's capabilities — falls back to observing-without-suppressing
//     if the grab or the virtual mirror can't be built, rather than
//     ever leaving a device grabbed with nothing forwarding its events.
//   - macOS: `macos.rs` uses a real CGEventTap (not just polling live key
//     state, which can't suppress anything) — returning
//     CallbackResult::Drop removes the event from the stream entirely.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MouseButton {
    Middle,
    Back,
    Forward,
}

impl MouseButton {
    pub fn parse(s: &str) -> Self {
        match s {
            "back" => MouseButton::Back,
            "forward" => MouseButton::Forward,
            _ => MouseButton::Middle,
        }
    }

    fn as_u8(self) -> u8 {
        match self {
            MouseButton::Middle => 0,
            MouseButton::Back => 1,
            MouseButton::Forward => 2,
        }
    }

    fn from_u8(v: u8) -> Self {
        match v {
            1 => MouseButton::Back,
            2 => MouseButton::Forward,
            _ => MouseButton::Middle,
        }
    }
}

static MOUSE_MODE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
static MOUSE_BUTTON: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
static MOUSE_DICTATION_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Updates the shared "Mouse mode" on/off flag. Call at boot (from the
/// loaded config) and whenever Settings saves a change to it. Resets the
/// toggle state so flipping it can't leave a stale "already active" flag
/// that would silently eat the next real click.
pub fn set_mouse_mode_enabled(on: bool) {
    use std::sync::atomic::Ordering;
    MOUSE_MODE.store(on, Ordering::SeqCst);
    MOUSE_DICTATION_ACTIVE.store(false, Ordering::SeqCst);
}

/// Updates the currently-configured mouse button. Call at boot and on
/// every save_config. Also resets the toggle state, for the same reason
/// as set_mouse_mode_enabled.
pub fn set_mouse_button(btn: MouseButton) {
    use std::sync::atomic::Ordering;
    MOUSE_BUTTON.store(btn.as_u8(), Ordering::SeqCst);
    MOUSE_DICTATION_ACTIVE.store(false, Ordering::SeqCst);
}

/// Whether a detected click on `btn` should actually be treated as a
/// mouse-mode trigger right now — Mouse mode is on AND it's the
/// currently-configured button. Platform watchers call this to decide
/// both whether to react at all, and (Windows only) whether to suppress
/// the click.
pub fn should_handle_mouse_click(btn: MouseButton) -> bool {
    use std::sync::atomic::Ordering;
    MOUSE_MODE.load(Ordering::SeqCst) && MouseButton::from_u8(MOUSE_BUTTON.load(Ordering::SeqCst)) == btn
}

/// Toggles dictation on a mouse-mode click: the first click starts
/// (forwards Pressed), the next stops (forwards Released). Always
/// toggles — unlike route_physical_event, this doesn't consult
/// "Tap to talk" at all, since a mouse click is never a hold.
pub fn route_mouse_click(tx: &Sender<HotkeyEvent>) {
    toggle_forward(
        &MOUSE_DICTATION_ACTIVE,
        HotkeyEvent::DictationPressed,
        HotkeyEvent::DictationReleased,
        tx,
    );
}

/// Thin public entry point for desktop.rs's boot sequence — `macos` is a
/// private submodule (platform internals stay out of the public API
/// surface, same as the rest of this file), so this is the one crack in
/// that wall, purely to spawn the watcher once at startup.
#[cfg(target_os = "macos")]
pub fn spawn_mac_mouse_mode_watcher(tx: Sender<HotkeyEvent>) {
    macos::spawn_mouse_mode_watcher(tx);
}

/// Parsed hotkey: required modifier state + non-modifier key.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ParsedHotkey {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub meta: bool,
    pub key: Option<String>,
}

impl ParsedHotkey {
    pub fn parse(s: &str) -> Self {
        let mut h = ParsedHotkey::default();
        for raw in s.split('+') {
            let part = raw.trim();
            if part.is_empty() {
                continue;
            }
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => h.ctrl = true,
                "shift" => h.shift = true,
                "alt" | "option" => h.alt = true,
                "meta" | "win" | "super" | "cmd" => h.meta = true,
                _ => h.key = Some(normalize_key_name(part)),
            }
        }
        h
    }

    /// True if this hotkey is a pure modifier chord (no non-modifier key)
    /// with at least two modifiers. RegisterHotKey can't represent these,
    /// so we watch them with a polling thread instead.
    pub fn is_modifier_chord(&self) -> bool {
        if self.key.is_some() {
            return false;
        }
        let count = [self.ctrl, self.shift, self.alt, self.meta]
            .iter()
            .filter(|b| **b)
            .count();
        count >= 2
    }
}

/// Hotkeys the listener watches simultaneously.
#[derive(Clone, Debug, Default)]
pub struct HotkeySet {
    pub dictation: ParsedHotkey,
    pub polish_dictation: ParsedHotkey,
    /// Per-transform slot bindings (transform_id, parsed hotkey).
    pub transform_bindings: Vec<(i64, ParsedHotkey)>,
}

fn normalize_key_name(s: &str) -> String {
    let trimmed = s.trim();
    // Compound names need a fixed canonical form so that saved configs
    // round-trip cleanly: file → ParsedHotkey::parse → this function →
    // key_name_to_code lookup. If we let the generic capitalise-first
    // logic run on "PageUp" it becomes "Pageup", and then the Code
    // match arm "PageUp" => Code::PageUp never fires.
    let lower = trimmed.to_ascii_lowercase();
    match lower.as_str() {
        "up" | "arrowup" => return "Up".into(),
        "down" | "arrowdown" => return "Down".into(),
        "left" | "arrowleft" => return "Left".into(),
        "right" | "arrowright" => return "Right".into(),
        "insert" | "ins" => return "Insert".into(),
        "delete" | "del" => return "Delete".into(),
        "home" => return "Home".into(),
        "end" => return "End".into(),
        "pageup" | "pgup" => return "PageUp".into(),
        "pagedown" | "pgdn" => return "PageDown".into(),
        "space" => return "Space".into(),
        "tab" => return "Tab".into(),
        "enter" | "return" => return "Enter".into(),
        "backspace" => return "Backspace".into(),
        "escape" | "esc" => return "Escape".into(),
        _ => {}
    }
    if trimmed.len() == 1 {
        return trimmed.to_ascii_uppercase();
    }
    let mut out = String::with_capacity(trimmed.len());
    let mut chars = trimmed.chars();
    if let Some(c) = chars.next() {
        out.push(c.to_ascii_uppercase());
    }
    for c in chars {
        out.push(c.to_ascii_lowercase());
    }
    if out.starts_with('F') && out[1..].chars().all(|c| c.is_ascii_digit()) {
        out = out.to_ascii_uppercase();
    }
    out
}

/// Convert our internal key string ("Space", "A", "F9") to the plugin's
/// `Code` (W3C UI Events `code` values). The plugin is cross-platform —
/// these mappings work on Windows + macOS + Linux.
fn key_name_to_code(name: &str) -> Option<Code> {
    Some(match name {
        "Space" => Code::Space,
        "Tab" => Code::Tab,
        "Return" | "Enter" => Code::Enter,
        "Backspace" => Code::Backspace,
        "Escape" => Code::Escape,
        "A" => Code::KeyA, "B" => Code::KeyB, "C" => Code::KeyC,
        "D" => Code::KeyD, "E" => Code::KeyE, "F" => Code::KeyF,
        "G" => Code::KeyG, "H" => Code::KeyH, "I" => Code::KeyI,
        "J" => Code::KeyJ, "K" => Code::KeyK, "L" => Code::KeyL,
        "M" => Code::KeyM, "N" => Code::KeyN, "O" => Code::KeyO,
        "P" => Code::KeyP, "Q" => Code::KeyQ, "R" => Code::KeyR,
        "S" => Code::KeyS, "T" => Code::KeyT, "U" => Code::KeyU,
        "V" => Code::KeyV, "W" => Code::KeyW, "X" => Code::KeyX,
        "Y" => Code::KeyY, "Z" => Code::KeyZ,
        "0" => Code::Digit0, "1" => Code::Digit1, "2" => Code::Digit2,
        "3" => Code::Digit3, "4" => Code::Digit4, "5" => Code::Digit5,
        "6" => Code::Digit6, "7" => Code::Digit7, "8" => Code::Digit8,
        "9" => Code::Digit9,
        "F1" => Code::F1, "F2" => Code::F2, "F3" => Code::F3,
        "F4" => Code::F4, "F5" => Code::F5, "F6" => Code::F6,
        "F7" => Code::F7, "F8" => Code::F8, "F9" => Code::F9,
        "F10" => Code::F10, "F11" => Code::F11, "F12" => Code::F12,
        "Up" => Code::ArrowUp,
        "Down" => Code::ArrowDown,
        "Left" => Code::ArrowLeft,
        "Right" => Code::ArrowRight,
        "Insert" => Code::Insert,
        "Delete" => Code::Delete,
        "Home" => Code::Home,
        "End" => Code::End,
        "PageUp" => Code::PageUp,
        "PageDown" => Code::PageDown,
        ";" => Code::Semicolon,
        "'" => Code::Quote,
        "," => Code::Comma,
        "." => Code::Period,
        "/" => Code::Slash,
        "\\" => Code::Backslash,
        "[" => Code::BracketLeft,
        "]" => Code::BracketRight,
        "-" => Code::Minus,
        "=" => Code::Equal,
        "`" => Code::Backquote,
        _ => return None,
    })
}

pub fn parsed_to_shortcut(h: &ParsedHotkey) -> Option<Shortcut> {
    let mut mods = Modifiers::empty();
    if h.ctrl {
        mods |= Modifiers::CONTROL;
    }
    if h.shift {
        mods |= Modifiers::SHIFT;
    }
    if h.alt {
        mods |= Modifiers::ALT;
    }
    if h.meta {
        mods |= Modifiers::SUPER;
    }
    let code = key_name_to_code(h.key.as_deref()?)?;
    Some(Shortcut::new(Some(mods), code))
}

/// Create the hotkey channel. Returns (tx, rx). The tx is stored in
/// AppState so re-registration after a settings change reuses the same
/// channel; the rx feeds the orchestrator.
pub fn make_channel() -> (Sender<HotkeyEvent>, Receiver<HotkeyEvent>) {
    mpsc::channel::<HotkeyEvent>()
}

/// Reported back to the frontend so the Transforms UI can show "Alt+3"
/// next to a card and dim it if the slot couldn't be registered (e.g.
/// another app already owns the combo).
#[derive(Clone, Debug, serde::Serialize)]
pub struct TransformSlotStatus {
    pub transform_id: i64,
    pub slot: u8, // 1..=9
    pub combo: String, // e.g. "Alt+3" — human-readable
    pub registered: bool,
    pub error: Option<String>,
}

/// Register the current hotkeys with the global-shortcut plugin, wiring
/// callbacks into the provided sender. Call again after a settings change
/// or a Transforms CRUD operation. The orchestrator's receiver does not
/// need to be rebuilt.
pub fn install_global_shortcuts(
    app: &AppHandle,
    set: Arc<Mutex<HotkeySet>>,
    tx: Sender<HotkeyEvent>,
) -> Vec<TransformSlotStatus> {
    re_register(app, &set, tx)
}

fn re_register(
    app: &AppHandle,
    set: &Arc<Mutex<HotkeySet>>,
    tx: Sender<HotkeyEvent>,
) -> Vec<TransformSlotStatus> {
    let gs = app.global_shortcut();
    if let Err(e) = gs.unregister_all() {
        tracing::warn!("unregister_all failed: {e:#}");
    }

    // Stop any platform-specific watchers from the previous registration
    // (Windows: clears the LL keyboard hook's chord mask; macOS: future
    // CGEventTap teardown). Always run, even if the new dictation hotkey
    // is also a modifier chord — a fresh registration with the up-to-date
    // spec gets installed below.
    native::stop_native_watchers();

    let snapshot = set.lock().clone();

    // Linux: the best hotkey path is reading /dev/input directly (evdev)
    // — instant, true hold-to-talk, works on every compositor. Available
    // once the user has input-device access (the .deb's input-group
    // grant + one relogin). When it's driving, it fully owns dictation +
    // polish and we deliberately DON'T also run the portal or plugin
    // watchers for them — running two mechanisms at once is what made the
    // hotkey fire erratically before.
    #[cfg(target_os = "linux")]
    let evdev_driving = linux_evdev::available();
    #[cfg(not(target_os = "linux"))]
    let evdev_driving = false;

    // Transform ids the evdev reader is watching (Linux/evdev only). Used
    // below to report slot status instead of the global-shortcut plugin,
    // which can't see keys on Wayland.
    #[cfg(target_os = "linux")]
    let evdev_transform_ids: Vec<i64> = if evdev_driving {
        linux_evdev::register(
            tx.clone(),
            snapshot.dictation.clone(),
            snapshot.polish_dictation.clone(),
            &snapshot.transform_bindings,
        )
    } else {
        linux_evdev::stop();
        Vec::new()
    };

    // Mouse mode: independent of the dictation hotkey's own path above —
    // it reads whichever mouse devices are available regardless of
    // whether the keyboard hotkey itself is using evdev or the portal.
    // Observe-only (see linux_mouse.rs); registered unconditionally
    // whenever a mouse is readable, since should_handle_mouse_click
    // gates on the live mouse_mode/mouse_button config either way.
    #[cfg(target_os = "linux")]
    if linux_mouse::available() {
        linux_mouse::register(tx.clone());
    } else {
        linux_mouse::stop();
    }

    // Linux Wayland WITHOUT evdev access (pre-relogin, AppImage): fall
    // back to the GlobalShortcuts portal for dictation + polish. Neither
    // the plugin nor the X11 poller can see global key state on Wayland.
    // On X11 sessions this is skipped and Linux uses the Windows paths.
    #[cfg(target_os = "linux")]
    let wayland_portal = !evdev_driving && crate::linux_env::is_wayland();
    #[cfg(not(target_os = "linux"))]
    let wayland_portal = false;

    #[cfg(target_os = "linux")]
    if wayland_portal {
        linux_portal::register(
            tx.clone(),
            snapshot.dictation.clone(),
            snapshot.polish_dictation.clone(),
        );
    } else {
        // No portal when evdev drives (or on X11) — stop any prior task.
        linux_portal::stop();
    }

    // Dictation, branch A: modifier-only chord (e.g. Ctrl+Win). The
    // plugin's RegisterHotKey backend can't represent these, so we hand
    // off to the platform's native implementation (Windows uses the LL
    // keyboard hook; macOS/Linux use polling watchers).
    if evdev_driving || wayland_portal {
        // Handled by evdev / portal above.
    } else if snapshot.dictation.is_modifier_chord() {
        native::spawn_modifier_chord_watcher(tx.clone(), snapshot.dictation.clone());
    }
    // Dictation, branch B: regular combo with a non-modifier key
    // (Ctrl+Shift+Space etc.). Uses the global-shortcut plugin for press,
    // and a platform-native release poller for the key-up edge.
    else if let Some(dict_sc) = parsed_to_shortcut(&snapshot.dictation) {
        let tx_dict = tx.clone();
        let dict_parsed = snapshot.dictation.clone();
        let dict_active = Arc::new(Mutex::new(false));
        let last_fire = Arc::new(Mutex::new(None::<Instant>));
        let handler = move |_app: &AppHandle, sc: &Shortcut, event: tauri_plugin_global_shortcut::ShortcutEvent| {
            if event.state() != ShortcutState::Pressed {
                return;
            }
            // Cooldown + already-active gate to ignore auto-repeat.
            {
                let mut active = dict_active.lock();
                if *active {
                    return;
                }
                let mut last = last_fire.lock();
                let cooled = last.map_or(true, |t: Instant| {
                    t.elapsed().as_millis() >= FIRE_COOLDOWN_MS
                });
                if !cooled {
                    return;
                }
                *active = true;
                *last = Some(Instant::now());
            }
            tracing::debug!("global-shortcut dictation pressed: {:?}", sc);
            route_physical_event(HotkeyEvent::DictationPressed, &tx_dict);

            // Spawn a one-shot poller that watches for release, sends the
            // release event, then clears `dict_active` so the next press
            // can fire again. Still runs in tap mode — route_physical_event
            // is what decides whether the release actually gets forwarded
            // or swallowed, not this poller.
            let tx_release = tx_dict.clone();
            let parsed = dict_parsed.clone();
            let dict_active_clone = dict_active.clone();
            thread::spawn(move || {
                let (poll_tx, poll_rx) = mpsc::channel();
                native::spawn_release_poller(poll_tx, parsed, HotkeyEvent::DictationReleased);
                if let Ok(evt) = poll_rx.recv() {
                    route_physical_event(evt, &tx_release);
                }
                *dict_active_clone.lock() = false;
            });
        };
        if let Err(e) = gs.on_shortcut(dict_sc, handler) {
            tracing::warn!(
                "register dictation hotkey failed (combo unsupported by RegisterHotKey?): {e:#}"
            );
            #[cfg(target_os = "linux")]
            crate::linux_env::emit_hotkey_status(
                "none",
                format!("Couldn't register the dictation hotkey: {e}"),
            );
        } else {
            tracing::info!("registered dictation shortcut: {:?}", snapshot.dictation);
            #[cfg(target_os = "linux")]
            crate::linux_env::emit_hotkey_status(
                "x11",
                "Dictation hotkey registered via X11.".to_string(),
            );
        }
    }

    // Polish-dictation hotkey: hold to record, releases like dictation but
    // the orchestrator forces CleanupMode::Polished on the pipeline so the
    // output is rewritten-for-clarity regardless of the user's global
    // cleanup mode. Same press/release-poller pattern as dictation,
    // parameterised on PolishDictationReleased. On Linux the evdev or
    // portal registration above already covers polish.
    if evdev_driving || wayland_portal {
        // Handled by evdev / portal above.
    } else if let Some(pol_sc) = parsed_to_shortcut(&snapshot.polish_dictation) {
        let tx_pol = tx.clone();
        let pol_parsed = snapshot.polish_dictation.clone();
        let pol_active = Arc::new(Mutex::new(false));
        let last_fire = Arc::new(Mutex::new(None::<Instant>));
        let handler = move |_app: &AppHandle, sc: &Shortcut, event: tauri_plugin_global_shortcut::ShortcutEvent| {
            if event.state() != ShortcutState::Pressed {
                return;
            }
            {
                let mut active = pol_active.lock();
                if *active {
                    return;
                }
                let mut last = last_fire.lock();
                let cooled = last.map_or(true, |t: Instant| {
                    t.elapsed().as_millis() >= FIRE_COOLDOWN_MS
                });
                if !cooled {
                    return;
                }
                *active = true;
                *last = Some(Instant::now());
            }
            tracing::debug!("global-shortcut polish-dictation pressed: {:?}", sc);
            route_physical_event(HotkeyEvent::PolishDictationPressed, &tx_pol);

            let tx_release = tx_pol.clone();
            let parsed = pol_parsed.clone();
            let pol_active_clone = pol_active.clone();
            thread::spawn(move || {
                let (poll_tx, poll_rx) = mpsc::channel();
                native::spawn_release_poller(poll_tx, parsed, HotkeyEvent::PolishDictationReleased);
                if let Ok(evt) = poll_rx.recv() {
                    route_physical_event(evt, &tx_release);
                }
                *pol_active_clone.lock() = false;
            });
        };
        if let Err(e) = gs.on_shortcut(pol_sc, handler) {
            tracing::warn!("register polish-dictation hotkey failed: {e:#}");
        } else {
            tracing::info!(
                "registered polish-dictation shortcut: {:?}",
                snapshot.polish_dictation
            );
        }
    }

    // Transform slot hotkeys (Alt+1..Alt+9 by default). Each one fires
    // TransformTriggered(id) on press. If registration fails (e.g. the
    // combo is owned by another app), we surface the error to the UI via
    // the returned status vec instead of crashing. No release polling
    // needed — these are tap-to-trigger, not hold-to-talk.
    let mut statuses: Vec<TransformSlotStatus> = Vec::new();
    for (transform_id, hk) in &snapshot.transform_bindings {
        let slot = derive_slot_number(hk).unwrap_or(0);
        let combo = format_combo(hk);

        // Linux with evdev driving: the transform chord is watched by the
        // evdev reader (registered above), not the global-shortcut plugin
        // — on Wayland the plugin registers "successfully" but never
        // fires. Report status from what evdev actually resolved.
        #[cfg(target_os = "linux")]
        if evdev_driving {
            let registered = evdev_transform_ids.contains(transform_id);
            statuses.push(TransformSlotStatus {
                transform_id: *transform_id,
                slot,
                combo,
                registered,
                error: if registered {
                    None
                } else {
                    Some("Transform needs a chord ending in a non-modifier key".into())
                },
            });
            continue;
        }

        let Some(shortcut) = parsed_to_shortcut(hk) else {
            statuses.push(TransformSlotStatus {
                transform_id: *transform_id,
                slot,
                combo,
                registered: false,
                error: Some("Combo not representable as a shortcut".into()),
            });
            continue;
        };
        let tx_t = tx.clone();
        let id_for_cb = *transform_id;
        let last_fire: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
        let handler = move |_app: &AppHandle,
                            _sc: &Shortcut,
                            event: tauri_plugin_global_shortcut::ShortcutEvent| {
            if event.state() != ShortcutState::Pressed {
                return;
            }
            let mut last = last_fire.lock();
            let cooled = last.map_or(true, |t: Instant| {
                t.elapsed().as_millis() >= FIRE_COOLDOWN_MS
            });
            if !cooled {
                return;
            }
            *last = Some(Instant::now());
            let _ = tx_t.send(HotkeyEvent::TransformTriggered(id_for_cb));
        };
        match gs.on_shortcut(shortcut, handler) {
            Ok(_) => {
                tracing::info!(
                    "registered transform slot: id={} combo={}",
                    transform_id,
                    combo
                );
                statuses.push(TransformSlotStatus {
                    transform_id: *transform_id,
                    slot,
                    combo,
                    registered: true,
                    error: None,
                });
            }
            Err(e) => {
                tracing::warn!(
                    "register transform slot id={} combo={} failed: {e:#}",
                    transform_id,
                    combo
                );
                statuses.push(TransformSlotStatus {
                    transform_id: *transform_id,
                    slot,
                    combo,
                    registered: false,
                    error: Some(format!("{e:#}")),
                });
            }
        }
    }
    statuses
}

/// Pull the slot number out of an Alt+N parsed hotkey. Returns 0 if the
/// shape isn't recognisable (UI then treats it as a custom combo).
fn derive_slot_number(h: &ParsedHotkey) -> Option<u8> {
    let key = h.key.as_deref()?;
    if key.len() != 1 {
        return None;
    }
    let c = key.chars().next()?;
    if c.is_ascii_digit() {
        Some(c as u8 - b'0')
    } else {
        None
    }
}

// The labels below are platform-aware: macOS glyphs (⌃⌥⇧⌘), Linux "Super",
// Windows "Win". The `combo` string is shown verbatim in the Transforms UI
// slot chips (TransformSlotStatus.combo → TransformCard). (The in-dashboard
// scratchpad hotkey routing is also handled now — see the
// "run-transform-in-app" emit in the TransformTriggered handler.)
//
// Per-transform hotkeys ARE user-editable (shipped 2026-07-19, a7b13e1):
// TransformsView's HotkeyRecorder writes a custom combo to Transform.hotkey,
// resolve_slot_hotkey (desktop.rs) reads it first and validates it before
// falling back to the numbered default (Alt+N / ⌘N) reflected here.
fn format_combo(h: &ParsedHotkey) -> String {
    // macOS shows shortcuts as glyphs with no separators (⌃⌥⇧⌘ + key), in
    // that canonical modifier order. Windows/Linux use "Mod+Mod+Key" — with
    // the meta key labelled "Win" on Windows and "Super" on Linux (never the
    // Windows-centric "Win" on Linux).
    #[cfg(target_os = "macos")]
    {
        let mut s = String::new();
        if h.ctrl {
            s.push('⌃');
        }
        if h.alt {
            s.push('⌥');
        }
        if h.shift {
            s.push('⇧');
        }
        if h.meta {
            s.push('⌘');
        }
        if let Some(k) = &h.key {
            s.push_str(k);
        }
        s
    }
    #[cfg(not(target_os = "macos"))]
    {
        let mut parts: Vec<String> = Vec::new();
        if h.ctrl {
            parts.push("Ctrl".into());
        }
        if h.shift {
            parts.push("Shift".into());
        }
        if h.alt {
            parts.push("Alt".into());
        }
        if h.meta {
            parts.push(if cfg!(target_os = "linux") { "Super" } else { "Win" }.into());
        }
        if let Some(k) = &h.key {
            parts.push(k.clone());
        }
        parts.join("+")
    }
}
