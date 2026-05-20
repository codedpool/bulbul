use parking_lot::Mutex;
use rdev::{listen, Event, EventType, Key};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;

#[derive(Clone, Debug)]
pub enum HotkeyEvent {
    Pressed,
    Released,
}

/// Parsed hotkey: required modifier state + optional non-modifier key.
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

    pub fn rdev_key(&self) -> Option<Key> {
        self.key.as_deref().and_then(key_name_to_rdev)
    }
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
        // F-keys always uppercase.
        if out.starts_with('F') && out[1..].chars().all(|c| c.is_ascii_digit()) {
            out = out.to_ascii_uppercase();
        }
        out
    }
}

fn key_name_to_rdev(name: &str) -> Option<Key> {
    match name {
        "Space" => Some(Key::Space),
        "Tab" => Some(Key::Tab),
        "Return" | "Enter" => Some(Key::Return),
        "Backspace" => Some(Key::Backspace),
        "Escape" => Some(Key::Escape),
        "A" => Some(Key::KeyA), "B" => Some(Key::KeyB), "C" => Some(Key::KeyC),
        "D" => Some(Key::KeyD), "E" => Some(Key::KeyE), "F" => Some(Key::KeyF),
        "G" => Some(Key::KeyG), "H" => Some(Key::KeyH), "I" => Some(Key::KeyI),
        "J" => Some(Key::KeyJ), "K" => Some(Key::KeyK), "L" => Some(Key::KeyL),
        "M" => Some(Key::KeyM), "N" => Some(Key::KeyN), "O" => Some(Key::KeyO),
        "P" => Some(Key::KeyP), "Q" => Some(Key::KeyQ), "R" => Some(Key::KeyR),
        "S" => Some(Key::KeyS), "T" => Some(Key::KeyT), "U" => Some(Key::KeyU),
        "V" => Some(Key::KeyV), "W" => Some(Key::KeyW), "X" => Some(Key::KeyX),
        "Y" => Some(Key::KeyY), "Z" => Some(Key::KeyZ),
        "0" => Some(Key::Num0), "1" => Some(Key::Num1), "2" => Some(Key::Num2),
        "3" => Some(Key::Num3), "4" => Some(Key::Num4), "5" => Some(Key::Num5),
        "6" => Some(Key::Num6), "7" => Some(Key::Num7), "8" => Some(Key::Num8),
        "9" => Some(Key::Num9),
        "F1" => Some(Key::F1), "F2" => Some(Key::F2), "F3" => Some(Key::F3),
        "F4" => Some(Key::F4), "F5" => Some(Key::F5), "F6" => Some(Key::F6),
        "F7" => Some(Key::F7), "F8" => Some(Key::F8), "F9" => Some(Key::F9),
        "F10" => Some(Key::F10), "F11" => Some(Key::F11), "F12" => Some(Key::F12),
        _ => None,
    }
}

/// Spawns the rdev listener on a dedicated thread.
/// Returns a Sender for hotkey updates (so the user can rebind at runtime)
/// and a Receiver of Pressed/Released events.
pub fn spawn_listener(initial: ParsedHotkey) -> (Arc<Mutex<ParsedHotkey>>, Receiver<HotkeyEvent>) {
    let (tx, rx) = mpsc::channel();
    let hotkey = Arc::new(Mutex::new(initial));
    let hotkey_inner = hotkey.clone();

    thread::spawn(move || {
        let pressed: Arc<Mutex<HashSet<Key>>> = Arc::new(Mutex::new(HashSet::new()));
        let active = Arc::new(Mutex::new(false));
        let tx: Sender<HotkeyEvent> = tx;

        if let Err(e) = listen(move |event: Event| {
            handle_event(&event, &pressed, &active, &hotkey_inner, &tx);
        }) {
            tracing::error!("rdev listener died: {e:?}");
        }
    });

    (hotkey, rx)
}

fn handle_event(
    event: &Event,
    pressed: &Arc<Mutex<HashSet<Key>>>,
    active: &Arc<Mutex<bool>>,
    hotkey: &Arc<Mutex<ParsedHotkey>>,
    tx: &Sender<HotkeyEvent>,
) {
    let (key, is_down) = match event.event_type {
        EventType::KeyPress(k) => (k, true),
        EventType::KeyRelease(k) => (k, false),
        _ => return,
    };

    {
        let mut p = pressed.lock();
        if is_down {
            p.insert(key);
        } else {
            p.remove(&key);
        }
    }

    let p = pressed.lock();
    let h = hotkey.lock();
    let now_matches = matches(&p, &h);
    drop(h);
    drop(p);

    let mut a = active.lock();
    if !*a && now_matches {
        *a = true;
        let _ = tx.send(HotkeyEvent::Pressed);
    } else if *a && !now_matches {
        *a = false;
        let _ = tx.send(HotkeyEvent::Released);
    }
}

fn matches(pressed: &HashSet<Key>, h: &ParsedHotkey) -> bool {
    let ctrl_d = pressed.contains(&Key::ControlLeft) || pressed.contains(&Key::ControlRight);
    let shift_d = pressed.contains(&Key::ShiftLeft) || pressed.contains(&Key::ShiftRight);
    let alt_d = pressed.contains(&Key::Alt) || pressed.contains(&Key::AltGr);
    let meta_d = pressed.contains(&Key::MetaLeft) || pressed.contains(&Key::MetaRight);

    if h.ctrl != ctrl_d || h.shift != shift_d || h.alt != alt_d || h.meta != meta_d {
        return false;
    }
    match h.rdev_key() {
        Some(k) => pressed.contains(&k),
        None => true,
    }
}
