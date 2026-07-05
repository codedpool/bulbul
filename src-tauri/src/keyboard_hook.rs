//! Global low-level keyboard hook for modifier-only chord dictation.
//!
//! Why this exists: Windows decides "open Start menu?" by checking whether
//! the Win key was used in combination with another key while held. Other
//! modifier keys (Ctrl, Alt, Shift) do not count, and synthetic events
//! injected via `SendInput` are tagged `LLKHF_INJECTED` and filtered out
//! of that check by the shell. So Ctrl+Win used as a hold-to-talk hotkey
//! invariably leaks: hold both, release Ctrl first, release Win, and
//! Start menu pops — pasting the dictated text into the search field.
//!
//! `WH_KEYBOARD_LL` lets us intercept keystrokes upstream of Windows's
//! shell. While a Bulbul chord is "engaged," we drop the chord-modifier
//! events (`LRESULT(1)`) so the shell never sees them. Windows literally
//! has no record of the Win press in a form that could trigger tap
//! detection. After the user lifts the chord, we sync Windows's key
//! state via a synthetic injected `KEYUP` — the shell ignores injected
//! events for tap detection (verified experimentally with `VK_F24`),
//! so this doesn't reopen the leak.
//!
//! The hook lives on a dedicated thread with a Windows message pump
//! because `SetWindowsHookExW(WH_KEYBOARD_LL, …)` requires one. The
//! callback writes to atomics + an `mpsc::Sender` that the dictation
//! orchestrator already drains; no new IPC machinery needed.

use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc::Sender;
use std::sync::OnceLock;
use std::thread;
use std::time::Duration;

use windows::Win32::Foundation::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::PCWSTR;

use crate::hotkey::{HotkeyEvent, ParsedHotkey};

// ─── Modifier bitmask ─────────────────────────────────────────────────

pub const MOD_CTRL: u8 = 1 << 0;
pub const MOD_SHIFT: u8 = 1 << 1;
pub const MOD_ALT: u8 = 1 << 2;
pub const MOD_META: u8 = 1 << 3;

/// Build a chord mask from a ParsedHotkey. Returns 0 if it isn't a
/// modifier-only chord (i.e. it has a non-modifier key, so RegisterHotKey
/// can handle it without our help).
pub fn chord_mask_for(h: &ParsedHotkey) -> u8 {
    if h.key.is_some() {
        return 0;
    }
    let mut m = 0u8;
    if h.ctrl {
        m |= MOD_CTRL;
    }
    if h.shift {
        m |= MOD_SHIFT;
    }
    if h.alt {
        m |= MOD_ALT;
    }
    if h.meta {
        m |= MOD_META;
    }
    m
}

// ─── Shared state with the hook callback ──────────────────────────────

/// Currently-held modifier set as seen by the hook (physical state, with
/// our own injected events filtered out). Updated on every keystroke.
static HELD_MODS: AtomicU8 = AtomicU8::new(0);

/// The chord mask we're watching for. 0 = no chord configured (hook is
/// dormant — it still observes keystrokes to keep `HELD_MODS` fresh but
/// never suppresses or fires events). Updated by `set_chord_mask`.
static CHORD_MASK: AtomicU8 = AtomicU8::new(0);

/// True while the configured chord is fully held. Drives DictationPressed
/// / DictationReleased events to the orchestrator.
static CHORD_ENGAGED: AtomicBool = AtomicBool::new(false);

/// One-shot OnceLock holding the mpsc sender from the orchestrator. The
/// callback fires events through this; it's wrapped in Mutex<Option<>>
/// so the sender can be swapped (e.g. across hot-reload during dev).
fn event_tx_slot() -> &'static Mutex<Option<Sender<HotkeyEvent>>> {
    static SLOT: OnceLock<Mutex<Option<Sender<HotkeyEvent>>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

fn send_event(evt: HotkeyEvent) {
    if let Some(tx) = event_tx_slot().lock().as_ref() {
        let _ = tx.send(evt);
    }
}

/// Has the keyboard hook thread already been spawned? Atomic so the
/// installer can be called multiple times (e.g. on settings reload)
/// without ever installing the hook twice.
static HOOK_INSTALLED: AtomicBool = AtomicBool::new(false);

// ─── Public API ───────────────────────────────────────────────────────

/// Install the global LL keyboard hook on a dedicated thread, idempotent.
/// Subsequent calls just refresh the event sender. Call once at app
/// startup; the orchestrator can later swap the sender if needed.
pub fn install(tx: Sender<HotkeyEvent>) {
    *event_tx_slot().lock() = Some(tx);
    if HOOK_INSTALLED.swap(true, Ordering::AcqRel) {
        return;
    }
    thread::Builder::new()
        .name("bulbul-kbd-hook".into())
        .spawn(|| unsafe { hook_thread_main() })
        .expect("spawn keyboard hook thread");
}

/// Set (or clear, with 0) the chord-modifier mask the hook should
/// watch for. Safe to call from any thread; takes effect on the next
/// keystroke. If a chord was engaged under the previous mask, the
/// orchestrator is notified of the release before the new mask kicks in.
pub fn set_chord_mask(mask: u8) {
    let prev = CHORD_MASK.swap(mask, Ordering::AcqRel);
    if prev == mask {
        return;
    }
    tracing::info!("keyboard_hook: chord mask set to 0b{:04b}", mask);
    // If we were mid-chord and the user is changing the binding, force a
    // synthetic release so the orchestrator doesn't get stuck holding a
    // recording open under the old chord.
    if CHORD_ENGAGED.swap(false, Ordering::AcqRel) {
        send_event(HotkeyEvent::DictationReleased);
    }
}

// ─── Hook thread ──────────────────────────────────────────────────────

unsafe fn hook_thread_main() {
    // Seed HELD_MODS from the OS's actual key state so we don't start
    // with a lie. If the user is holding a modifier at the instant our
    // hook comes up (autostart-at-login, lock-screen unlock with a key
    // still pressed, etc.), starting at 0 means our observed state
    // diverges from reality immediately and stays divergent until the
    // user happens to release that modifier — which can take hours.
    let seed = current_real_modifier_state();
    HELD_MODS.store(seed, Ordering::Release);
    tracing::info!("keyboard_hook: initial HELD_MODS seeded to 0b{:04b}", seed);

    let h_mod = match GetModuleHandleW(PCWSTR::null()) {
        Ok(h) => HINSTANCE(h.0),
        Err(e) => {
            tracing::error!("keyboard_hook: GetModuleHandleW failed: {e:?}");
            return;
        }
    };
    let hook = match SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook_proc), h_mod, 0) {
        Ok(h) => h,
        Err(e) => {
            tracing::error!("keyboard_hook: SetWindowsHookExW failed: {e:?}");
            return;
        }
    };
    tracing::info!("keyboard_hook: WH_KEYBOARD_LL installed (HHOOK={:?})", hook.0);

    // Pump messages so the hook stays alive. GetMessageW blocks; the
    // OS posts hook messages here as keystrokes flow through. We never
    // post a quit message — the hook lives for the process lifetime.
    let mut msg = MSG::default();
    while GetMessageW(&mut msg, HWND::default(), 0, 0).as_bool() {
        let _ = TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
    tracing::info!("keyboard_hook: message loop exited");
}

// ─── Callback ─────────────────────────────────────────────────────────

unsafe extern "system" fn keyboard_hook_proc(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // Per MSDN: if `code < 0`, just forward — we MUST NOT process.
    if code != HC_ACTION as i32 {
        return CallNextHookEx(None, code, wparam, lparam);
    }
    let kb = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
    let msg = wparam.0 as u32;
    let is_keydown = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
    let is_keyup = msg == WM_KEYUP || msg == WM_SYSKEYUP;
    // Skip our own synthetic events. We tag them via SendInput, and the
    // shell tags them with LLKHF_INJECTED. Reprocessing them here would
    // mean Win-UP we just injected to sync state gets suppressed again
    // and we never reach equilibrium.
    let injected = (kb.flags.0 & LLKHF_INJECTED.0) != 0;
    if injected {
        return CallNextHookEx(None, code, wparam, lparam);
    }

    // Update HELD_MODS for modifier keys. Track L+R variants so we
    // don't lose a held state when only the other side is released.
    let vk = kb.vkCode;
    let mod_bit: u8 = match vk {
        v if v == VK_LCONTROL.0 as u32 || v == VK_RCONTROL.0 as u32 => MOD_CTRL,
        v if v == VK_LSHIFT.0 as u32 || v == VK_RSHIFT.0 as u32 => MOD_SHIFT,
        v if v == VK_LMENU.0 as u32 || v == VK_RMENU.0 as u32 => MOD_ALT,
        v if v == VK_LWIN.0 as u32 || v == VK_RWIN.0 as u32 => MOD_META,
        _ => 0,
    };

    let mask = CHORD_MASK.load(Ordering::Acquire);

    // Snapshot state BEFORE updating, so we can detect transitions
    // (engaged ↔ released) by comparing old vs new modifier sets.
    let prev_held = HELD_MODS.load(Ordering::Acquire);
    let new_held = if mod_bit != 0 && is_keydown {
        prev_held | mod_bit
    } else if mod_bit != 0 && is_keyup {
        prev_held & !mod_bit
    } else {
        prev_held
    };
    if new_held != prev_held {
        HELD_MODS.store(new_held, Ordering::Release);
    }

    // With no chord configured the hook is purely an observer. Fall
    // through to the OS so other apps keep working normally.
    if mask == 0 {
        return CallNextHookEx(None, code, wparam, lparam);
    }

    let was_engaged = CHORD_ENGAGED.load(Ordering::Acquire);
    let now_engaged = (new_held & mask) == mask && mask != 0;
    let is_chord_mod = (mod_bit & mask) != 0;

    // Engage transition: this event completed the chord. Suppress it so
    // the OS never sees the final modifier press going down — most
    // importantly the Win press, which is what arms Start-menu tap
    // detection on release.
    //
    // Before engaging we cross-check against the OS's actual key state
    // (`GetAsyncKeyState`). HELD_MODS only reflects events the hook has
    // observed; any missed event (session lock swallowing a Win-UP,
    // autostart racing the user's modifier release, another process
    // injecting then dropping a modifier event) leaves us with a
    // permanently-set bit that turns the very next chord-modifier press
    // into a false engagement. Witnessed in the wild: after a Win+L
    // lock, the next solo Ctrl press would start dictation and break
    // Ctrl+A / Ctrl+V system-wide. If reality says the chord modifiers
    // aren't all really held, resync HELD_MODS to match and skip the
    // engagement.
    if !was_engaged && now_engaged {
        let real = current_real_modifier_state();
        if (real & mask) != mask {
            tracing::warn!(
                "keyboard_hook: false engagement averted — HELD_MODS=0b{:04b} but real=0b{:04b}, mask=0b{:04b}. Resyncing.",
                new_held,
                real,
                mask
            );
            HELD_MODS.store(real, Ordering::Release);
            return CallNextHookEx(None, code, wparam, lparam);
        }
        CHORD_ENGAGED.store(true, Ordering::Release);
        tracing::debug!("keyboard_hook: chord engaged → sending DictationPressed");
        send_event(HotkeyEvent::DictationPressed);
        if is_chord_mod {
            return LRESULT(1);
        }
    }

    // Release transition: this event broke the chord. Suppress it
    // (it's a chord-modifier KEYUP), notify the orchestrator, and
    // schedule a state sync to release the Win key cleanly in Windows's
    // own view of the world.
    if was_engaged && !now_engaged {
        CHORD_ENGAGED.store(false, Ordering::Release);
        send_event(HotkeyEvent::DictationReleased);
        if is_chord_mod {
            sync_chord_modifier_state_async(mask);
            return LRESULT(1);
        }
    }

    // Engaged steady-state: keep suppressing any chord-modifier traffic
    // so repeated KEYDOWN auto-repeat or a side-by-side L/R modifier
    // (e.g. user releases LCTRL while still holding RCTRL) doesn't
    // leak through.
    if was_engaged && is_chord_mod {
        return LRESULT(1);
    }

    CallNextHookEx(None, code, wparam, lparam)
}

// ─── Win-key state sync ───────────────────────────────────────────────

/// After we suppress the user's physical Win/Alt KEYUP, Windows's
/// internal key state still thinks the key is held (we never let the
/// release event reach the OS). Inject a synthetic release so the OS
/// catches up. The synthetic event carries `LLKHF_INJECTED`, which the
/// shell filters out of its Start-menu tap-detection — so the sync
/// doesn't reopen the very leak we just closed.
///
/// We only do this for Win (and Alt, for the parallel browser-menubar
/// problem). Ctrl/Shift releases are harmless in isolation, so we skip
/// them to keep the synthetic event burst minimal.
fn sync_chord_modifier_state_async(mask: u8) {
    // Off-thread because SendInput can re-entrantly trigger our own
    // hook even with LLKHF_INJECTED — better to keep the callback hot
    // path tiny.
    thread::spawn(move || {
        // Tiny pause so the real keyup we suppressed is fully processed
        // by the input subsystem before our synthetic events land.
        thread::sleep(Duration::from_millis(5));
        let mut events: Vec<INPUT> = Vec::new();
        if (mask & MOD_META) != 0 {
            events.push(key_up_input(VK_LWIN));
            events.push(key_up_input(VK_RWIN));
        }
        if (mask & MOD_ALT) != 0 {
            events.push(key_up_input(VK_MENU));
        }
        if events.is_empty() {
            return;
        }
        let sent = unsafe { SendInput(&events, std::mem::size_of::<INPUT>() as i32) };
        if sent as usize != events.len() {
            tracing::warn!(
                "keyboard_hook: state sync SendInput delivered {sent}/{}",
                events.len()
            );
        }
    });
}

/// Read modifier keys' actual physical state from the OS, as a chord-mask
/// bitset. Used to cross-check HELD_MODS at engagement time and to seed
/// it at hook startup. `GetAsyncKeyState`'s high bit (`0x8000`) signals
/// "currently down" and reflects kernel-level state regardless of our
/// suppression activity, which is exactly what we want as a ground
/// truth.
fn current_real_modifier_state() -> u8 {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
    };
    fn down(vk: i32) -> bool {
        (unsafe { GetAsyncKeyState(vk) } as u16 & 0x8000) != 0
    }
    let mut m = 0u8;
    if down(VK_CONTROL.0 as i32) {
        m |= MOD_CTRL;
    }
    if down(VK_SHIFT.0 as i32) {
        m |= MOD_SHIFT;
    }
    if down(VK_MENU.0 as i32) {
        m |= MOD_ALT;
    }
    if down(VK_LWIN.0 as i32) || down(VK_RWIN.0 as i32) {
        m |= MOD_META;
    }
    m
}

fn key_up_input(vk: VIRTUAL_KEY) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: KEYEVENTF_KEYUP,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}
