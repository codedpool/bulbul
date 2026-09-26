// SPDX-License-Identifier: GPL-3.0-only
// Copyright (c) 2026 Romanch Roshan Singh

//! Global low-level mouse hook for "Mouse mode" — a configurable mouse
//! button (default: middle-click) that toggles dictation on and off.
//!
//! Sibling to `keyboard_hook.rs`, deliberately kept separate rather than
//! merged into it: mouse-mode has none of the keyboard hook's chord/hold
//! complexity (no modifier combinations, no "engaged" steady-state, no
//! Start-menu tap-detection leak to guard against) — it only ever reacts
//! to a single button's DOWN edge and always toggles, via
//! `hotkey::route_mouse_click`. Keeping this in its own file, with its own
//! independent lifecycle, means the proven keyboard hook is never touched
//! by this feature.
//!
//! `WH_MOUSE_LL` sits inline in the delivery path the same way
//! `WH_KEYBOARD_LL` does, so returning a nonzero value from the hook
//! genuinely suppresses the click system-wide — this is what makes the
//! configured button's normal effect (browser back/forward, etc.) not
//! fire while Mouse mode is on, exactly like the keyboard hook suppresses
//! Ctrl+Win.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Mutex, OnceLock};
use std::thread;

use windows::Win32::Foundation::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::PCWSTR;

use crate::hotkey::{self, HotkeyEvent, MouseButton};

fn event_tx_slot() -> &'static Mutex<Option<Sender<HotkeyEvent>>> {
    static SLOT: OnceLock<Mutex<Option<Sender<HotkeyEvent>>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

static HOOK_THREAD_SPAWNED: AtomicBool = AtomicBool::new(false);

/// Install the global LL mouse hook on a dedicated thread, idempotent.
/// Subsequent calls just refresh the event sender. Call once at app
/// startup, alongside `keyboard_hook::install`.
pub fn install(tx: Sender<HotkeyEvent>) {
    *event_tx_slot().lock().unwrap() = Some(tx);
    if HOOK_THREAD_SPAWNED.swap(true, Ordering::AcqRel) {
        return;
    }
    thread::Builder::new()
        .name("bulbul-mouse-hook".into())
        .spawn(|| unsafe { hook_thread_main() })
        .expect("spawn mouse hook thread");
}

unsafe fn hook_thread_main() {
    let h_mod = match GetModuleHandleW(PCWSTR::null()) {
        Ok(h) => HINSTANCE(h.0),
        Err(e) => {
            tracing::error!("mouse_hook: GetModuleHandleW failed: {e:?}");
            return;
        }
    };

    let mut hook = install_hook(h_mod);
    match hook {
        Some(h) => tracing::info!("mouse_hook: WH_MOUSE_LL installed (HHOOK={:?})", h.0),
        None => tracing::error!("mouse_hook: initial SetWindowsHookExW failed — retrying on watchdog"),
    }

    // Same self-healing watchdog rationale as keyboard_hook: Windows can
    // silently drop a LL hook (timeout, session switch, sleep-resume)
    // with no notification, so we periodically re-assert it.
    const WATCHDOG_MS: u32 = 5_000;
    let timer_id = SetTimer(HWND::default(), 0, WATCHDOG_MS, None);

    let mut msg = MSG::default();
    while GetMessageW(&mut msg, HWND::default(), 0, 0).as_bool() {
        if msg.message == WM_TIMER && msg.wParam.0 == timer_id {
            let had_hook = hook.is_some();
            if let Some(h) = hook.take() {
                let _ = UnhookWindowsHookEx(h);
            }
            hook = install_hook(h_mod);
            match (had_hook, hook.is_some()) {
                (false, true) => tracing::info!("mouse_hook: re-installed after a drop"),
                (true, false) => tracing::warn!("mouse_hook: re-install FAILED"),
                _ => {}
            }
            continue;
        }
        let _ = TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }

    if let Some(h) = hook {
        let _ = UnhookWindowsHookEx(h);
    }
    let _ = KillTimer(HWND::default(), timer_id);
    tracing::info!("mouse_hook: message loop exited");
}

unsafe fn install_hook(h_mod: HINSTANCE) -> Option<HHOOK> {
    SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook_proc), h_mod, 0).ok()
}

unsafe extern "system" fn mouse_hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code != HC_ACTION as i32 {
        return CallNextHookEx(None, code, wparam, lparam);
    }
    let ms = &*(lparam.0 as *const MSLLHOOKSTRUCT);
    // Skip our own synthetic events, if we ever inject mouse input — none
    // today, but this matches keyboard_hook's defensive filtering rather
    // than assuming that stays true forever.
    if (ms.flags & LLMHF_INJECTED) != 0 {
        return CallNextHookEx(None, code, wparam, lparam);
    }

    let msg = wparam.0 as u32;
    let btn = match msg {
        m if m == WM_MBUTTONDOWN => Some(MouseButton::Middle),
        m if m == WM_XBUTTONDOWN => {
            // HIWORD(mouseData) carries XBUTTON1 (0x0001) or XBUTTON2 (0x0002).
            let x = ((ms.mouseData >> 16) & 0xFFFF) as u16;
            if x == XBUTTON1 as u16 {
                Some(MouseButton::Back)
            } else if x == XBUTTON2 as u16 {
                Some(MouseButton::Forward)
            } else {
                None
            }
        }
        _ => None,
    };

    if let Some(btn) = btn {
        if hotkey::should_handle_mouse_click(btn) {
            if let Some(tx) = event_tx_slot().lock().unwrap().as_ref() {
                hotkey::route_mouse_click(tx);
            }
            return LRESULT(1);
        }
    }

    CallNextHookEx(None, code, wparam, lparam)
}
