use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use tauri::AppHandle;
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

/// Minimum gap between two fires of the same hotkey. Guards against
/// auto-repeat and spurious event bursts.
const FIRE_COOLDOWN_MS: u128 = 700;
/// How often the release poller checks the dictation hotkey's key state.
const RELEASE_POLL_MS: u64 = 25;
/// How long both modifiers of a modifier-only chord (e.g. Ctrl+Win) must
/// be held before we treat it as a dictation request. Short enough to feel
/// snappy, long enough that brushing both keys for unrelated reasons
/// (Ctrl+Win+arrow to switch desktop, etc.) doesn't fire dictation.
const MODIFIER_CHORD_DEBOUNCE_MS: u64 = 80;

#[derive(Clone, Debug)]
pub enum HotkeyEvent {
    DictationPressed,
    DictationReleased,
    PolishDictationPressed,
    PolishDictationReleased,
    TransformTriggered(i64),
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

    pub fn is_meaningful(&self) -> bool {
        self.key.is_some() || self.ctrl || self.shift || self.alt || self.meta
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
    if trimmed.len() == 1 {
        trimmed.to_ascii_uppercase()
    } else {
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
}

/// Convert our internal key string ("Space", "A", "F9") to the plugin's
/// `Code` (W3C UI Events `code` values).
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
        _ => return None,
    })
}

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

fn is_key_down(vk: i32) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
    unsafe { GetAsyncKeyState(vk) < 0 }
}

/// Read the currently-held state of every modifier we care about.
/// Returns (ctrl, shift, alt, meta). VK_CONTROL/VK_SHIFT/VK_MENU are the
/// "either left or right" virtual keys; Win has separate L/R so we OR them.
fn modifier_state() -> (bool, bool, bool, bool) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
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

/// Long-running watcher for a modifier-only chord like Ctrl+Win. Polls
/// modifier state every 25ms. Fires DictationPressed once both required
/// modifiers have been held for MODIFIER_CHORD_DEBOUNCE_MS, and
/// DictationReleased the instant either is released. Stops when `alive`
/// is set to false (settings change → re_register swaps in a new watcher).
fn spawn_modifier_chord_watcher(
    tx: Sender<HotkeyEvent>,
    hotkey: ParsedHotkey,
    alive: Arc<AtomicBool>,
) {
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
        tracing::info!(
            "modifier-chord watcher started for {:?}",
            hotkey
        );
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
                        tracing::debug!(
                            "modifier-chord dictation pressed: {:?}",
                            hotkey
                        );
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

/// Holds the AtomicBool that keeps the current modifier-chord watcher
/// alive. re_register swaps in a fresh flag and signals the old one to
/// stop, so only one watcher runs at a time.
fn modifier_chord_alive_slot() -> &'static Mutex<Option<Arc<AtomicBool>>> {
    static SLOT: OnceLock<Mutex<Option<Arc<AtomicBool>>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// Hold-to-talk release detector. RegisterHotKey only signals key-down;
/// we poll the actual key state to detect when the user lets go. The
/// `release_evt` parameter lets the same poller drive either the
/// dictation pipeline or the voice-transform pipeline.
fn spawn_release_poller(tx: Sender<HotkeyEvent>, hotkey: ParsedHotkey, release_evt: HotkeyEvent) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
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

    // Stop any modifier-chord watcher from the previous registration. We
    // always swap, even if the new dictation hotkey is also a modifier
    // chord — a new watcher with the up-to-date spec gets spawned below.
    {
        let mut slot = modifier_chord_alive_slot().lock();
        if let Some(prev) = slot.take() {
            prev.store(false, Ordering::Relaxed);
        }
    }

    let snapshot = set.lock().clone();

    // Dictation, branch A: modifier-only chord (e.g. Ctrl+Win). The
    // plugin's RegisterHotKey backend can't represent these, so we run a
    // background polling thread that watches the modifier state directly.
    if snapshot.dictation.is_modifier_chord() {
        let alive = Arc::new(AtomicBool::new(true));
        *modifier_chord_alive_slot().lock() = Some(alive.clone());
        spawn_modifier_chord_watcher(tx.clone(), snapshot.dictation.clone(), alive);
        tracing::info!(
            "registered dictation (modifier-chord watcher): {:?}",
            snapshot.dictation
        );
    }
    // Dictation, branch B: regular combo with a non-modifier key
    // (Ctrl+Shift+Space etc.). Uses the global-shortcut plugin for press,
    // and a one-shot release poller for the key-up edge.
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
            let _ = tx_dict.send(HotkeyEvent::DictationPressed);

            // Spawn a one-shot poller that watches for release, sends the
            // release event, then clears `dict_active` so the next press
            // can fire again.
            let tx_release = tx_dict.clone();
            let parsed = dict_parsed.clone();
            let dict_active_clone = dict_active.clone();
            thread::spawn(move || {
                let (poll_tx, poll_rx) = mpsc::channel();
                spawn_release_poller(poll_tx, parsed, HotkeyEvent::DictationReleased);
                if let Ok(evt) = poll_rx.recv() {
                    let _ = tx_release.send(evt);
                }
                *dict_active_clone.lock() = false;
            });
        };
        if let Err(e) = gs.on_shortcut(dict_sc, handler) {
            tracing::warn!(
                "register dictation hotkey failed (combo unsupported by RegisterHotKey?): {e:#}"
            );
        } else {
            tracing::info!("registered dictation shortcut: {:?}", snapshot.dictation);
        }
    }

    // Polish-dictation hotkey: hold to record, releases like dictation but
    // the orchestrator forces CleanupMode::Polished on the pipeline so the
    // output is rewritten-for-clarity regardless of the user's global
    // cleanup mode. Same press/release-poller pattern as dictation,
    // parameterised on PolishDictationReleased.
    if let Some(pol_sc) = parsed_to_shortcut(&snapshot.polish_dictation) {
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
            let _ = tx_pol.send(HotkeyEvent::PolishDictationPressed);

            let tx_release = tx_pol.clone();
            let parsed = pol_parsed.clone();
            let pol_active_clone = pol_active.clone();
            thread::spawn(move || {
                let (poll_tx, poll_rx) = mpsc::channel();
                spawn_release_poller(poll_tx, parsed, HotkeyEvent::PolishDictationReleased);
                if let Ok(evt) = poll_rx.recv() {
                    let _ = tx_release.send(evt);
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
    // the returned status vec instead of crashing.
    let mut statuses: Vec<TransformSlotStatus> = Vec::new();
    for (transform_id, hk) in &snapshot.transform_bindings {
        let slot = derive_slot_number(hk).unwrap_or(0);
        let combo = format_combo(hk);
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

fn format_combo(h: &ParsedHotkey) -> String {
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
        parts.push("Win".into());
    }
    if let Some(k) = &h.key {
        parts.push(k.clone());
    }
    parts.join("+")
}
