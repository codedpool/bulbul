//! Wayland-native global hotkeys via the GlobalShortcuts portal.
//!
//! Wayland compositors deliberately refuse global key grabs — the only
//! sanctioned path is `org.freedesktop.portal.GlobalShortcuts`, which
//! hands press AND release signals (Activated/Deactivated), exactly
//! the pair our hold-to-talk model needs. The compositor shows the
//! user a one-time approval dialog listing the shortcuts; bindings are
//! remembered per app-id afterwards.
//!
//! Coverage is compositor-dependent: KDE Plasma 6 implements it fully;
//! GNOME ships it from 45 but newer releases reject non-sandboxed
//! callers. When binding fails we emit a `linux-hotkey-status` event
//! with backend "none" so the dashboard banner can point the user at
//! the universal fallback: a DE-level custom shortcut running
//! `bulbul --toggle-dictation` (see `cli_toggle_dictation` in lib.rs).
//!
//! Modifier-only chords (Ctrl+Win) are not representable as portal
//! triggers — a trigger needs a real key. `preferred_trigger` is only
//! a hint anyway; the user can rebind in the approval dialog, and the
//! compositor's choice wins.
//!
//! Re-registration discipline: registrations are strictly sequenced —
//! the new task awaits the previous task's shutdown (which closes its
//! portal session) before opening a fresh session. If a stuck approval
//! dialog holds the old task past the grace timeout we proceed anyway;
//! a briefly-overlapping session can only produce duplicate events,
//! which the orchestrator already absorbs (press-while-active and
//! release-while-idle are both no-ops there).

use std::sync::mpsc::Sender;
use std::sync::Mutex;
use std::time::Duration;

use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
use futures_util::StreamExt;

use super::{HotkeyEvent, ParsedHotkey};

const ID_DICTATION: &str = "dictation";
const ID_POLISH: &str = "polish";

type StopSender = tokio::sync::oneshot::Sender<()>;
type TaskHandle = tauri::async_runtime::JoinHandle<()>;

/// Previous registration's stop signal + join handle, so the next
/// registration can shut it down and wait for its session to close.
static PREV_SLOT: Mutex<Option<(StopSender, TaskHandle)>> = Mutex::new(None);

pub fn stop() {
    if let Some((stop_tx, _handle)) = PREV_SLOT.lock().unwrap().take() {
        let _ = stop_tx.send(());
    }
}

/// Map a parsed hotkey to the XDG shortcuts-spec trigger string the
/// portal expects, e.g. Ctrl+Alt+Space → "CTRL+ALT+space". Returns None
/// for modifier-only chords (no main key → not representable).
fn preferred_trigger(h: &ParsedHotkey) -> Option<String> {
    let key = h.key.as_deref()?;
    // xkb keysym names for the keys our recorder can produce. Letters
    // and digits are their lowercase selves; specials map per keysym
    // table. This is a *preference hint* — a miss just means the
    // approval dialog opens without a pre-filled binding.
    let keysym: String = match key {
        "Space" => "space".into(),
        "Tab" => "Tab".into(),
        "Return" | "Enter" => "Return".into(),
        "Backspace" => "BackSpace".into(),
        "Escape" => "Escape".into(),
        "Up" => "Up".into(),
        "Down" => "Down".into(),
        "Left" => "Left".into(),
        "Right" => "Right".into(),
        "Home" => "Home".into(),
        "End" => "End".into(),
        "PageUp" => "Prior".into(),
        "PageDown" => "Next".into(),
        "Insert" => "Insert".into(),
        "Delete" => "Delete".into(),
        ";" => "semicolon".into(),
        "'" => "apostrophe".into(),
        "," => "comma".into(),
        "." => "period".into(),
        "/" => "slash".into(),
        "\\" => "backslash".into(),
        "[" => "bracketleft".into(),
        "]" => "bracketright".into(),
        "-" => "minus".into(),
        "=" => "equal".into(),
        "`" => "grave".into(),
        k if k.len() == 1 => k.to_ascii_lowercase(),
        k => k.to_string(), // F1..F12 and friends pass through as-is
    };
    let mut parts: Vec<String> = Vec::new();
    if h.ctrl {
        parts.push("CTRL".into());
    }
    if h.shift {
        parts.push("SHIFT".into());
    }
    if h.alt {
        parts.push("ALT".into());
    }
    if h.meta {
        parts.push("LOGO".into());
    }
    parts.push(keysym);
    Some(parts.join("+"))
}

/// Longest a single hold-to-talk recording can run before the watchdog
/// force-releases it (guarding a lost Deactivated signal). Generous
/// enough not to clip a real long dictation, bounded enough that a
/// stuck listener recovers — and caps the audio sent to Groq.
const MAX_HOLD_SECS: u64 = 45;

/// Arm a cancellable timer that fires `release_evt` after MAX_HOLD_SECS
/// unless cancelled first (via the returned sender) by a real
/// Deactivated. Clears `active` when it fires so the event loop's
/// toggle-tolerance doesn't misread the NEXT press as a release.
fn arm_release_watchdog(
    tx: Sender<HotkeyEvent>,
    release_evt: HotkeyEvent,
    active: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> tokio::sync::oneshot::Sender<()> {
    let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
    tauri::async_runtime::spawn(async move {
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_secs(MAX_HOLD_SECS)) => {
                tracing::warn!(
                    "portal Deactivated never arrived within {MAX_HOLD_SECS}s — \
                     force-releasing (GNOME release-signal quirk)"
                );
                active.store(false, std::sync::atomic::Ordering::SeqCst);
                let _ = tx.send(release_evt);
            }
            _ = cancel_rx => { /* real release landed; nothing to do */ }
        }
    });
    cancel_tx
}

/// Register dictation + polish with the portal and pump its signal
/// streams into the orchestrator channel. Replaces any previous portal
/// registration. Failures are reported via `linux-hotkey-status` —
/// never fatal.
pub fn register(tx: Sender<HotkeyEvent>, dictation: ParsedHotkey, polish: ParsedHotkey) {
    let prev = PREV_SLOT.lock().unwrap().take();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();

    let handle = tauri::async_runtime::spawn(async move {
        // Sequence with the previous registration: signal it, then wait
        // for its session to actually close. Grace-capped so a stuck
        // approval dialog can't wedge re-registration forever.
        if let Some((prev_stop, prev_handle)) = prev {
            let _ = prev_stop.send(());
            let _ = tokio::time::timeout(Duration::from_secs(5), prev_handle).await;
        }
        match run_session(tx, dictation, polish, stop_rx).await {
            Ok(()) => {}
            Err(e) => {
                tracing::warn!("GlobalShortcuts portal unavailable: {e:#}");
                crate::linux_env::emit_hotkey_status(
                    "none",
                    format!(
                        "The Wayland global-shortcut portal isn't available on this desktop \
                         ({e}). Bind a system shortcut to Bulbul's CLI toggle instead.",
                    ),
                );
            }
        }
    });

    *PREV_SLOT.lock().unwrap() = Some((stop_tx, handle));
}

async fn run_session(
    tx: Sender<HotkeyEvent>,
    dictation: ParsedHotkey,
    polish: ParsedHotkey,
    stop_rx: tokio::sync::oneshot::Receiver<()>,
) -> Result<(), ashpd::Error> {
    // Attach our app id first. GNOME's portal rejects bind_shortcuts
    // with "an app id is required" (the exact error the tester saw)
    // unless the process is registered via the Registry interface.
    crate::linux_env::ensure_host_app_registered().await;

    let portal = GlobalShortcuts::new().await?;
    let session = portal.create_session(Default::default()).await?;

    let dictation_trigger = preferred_trigger(&dictation);
    let polish_trigger = preferred_trigger(&polish);
    let wanted = vec![
        NewShortcut::new(ID_DICTATION, "Bulbul — hold to dictate")
            .preferred_trigger(dictation_trigger.as_deref()),
        NewShortcut::new(ID_POLISH, "Bulbul — hold to dictate (polished rewrite)")
            .preferred_trigger(polish_trigger.as_deref()),
    ];

    let request = portal
        .bind_shortcuts(&session, &wanted, None, Default::default())
        .await?;
    let bound = request.response()?;
    let summary: Vec<String> = bound
        .shortcuts()
        .iter()
        .map(|s| format!("{}: {}", s.id(), s.trigger_description()))
        .collect();
    crate::linux_env::emit_hotkey_status(
        "portal",
        if summary.is_empty() {
            "Shortcuts registered with the compositor.".to_string()
        } else {
            summary.join(" · ")
        },
    );
    tracing::info!("GlobalShortcuts portal bound: {summary:?}");

    let activated = portal.receive_activated().await?;
    let deactivated = portal.receive_deactivated().await?;
    // The streams are opaque `impl Stream` without an Unpin guarantee;
    // pin them to the stack so StreamExt::next works in the loop.
    futures_util::pin_mut!(activated, deactivated);
    let mut stop_rx = stop_rx;

    // Release watchdog + toggle tolerance. GNOME's GlobalShortcuts
    // implementation is documented as unreliable about the Deactivated
    // (key-up) signal — on affected versions a held shortcut only ever
    // emits Activated, so recording never stops until the user presses
    // the hotkey AGAIN (which emits another Activated). Two defenses:
    //
    //   1. Toggle tolerance: an Activated that arrives while that
    //      shortcut is already active is treated as the missing release.
    //      On compositors with proper press/release (KDE) this branch
    //      never triggers; on GNOME the hotkey degrades gracefully to
    //      press-to-start / press-to-stop instead of press-and-pray.
    //   2. Watchdog: per press, a cancellable timer force-releases after
    //      MAX_HOLD_SECS in case neither Deactivated nor a second press
    //      ever shows up. Double-releases are no-ops in the orchestrator.
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    let dict_active = Arc::new(AtomicBool::new(false));
    let polish_active = Arc::new(AtomicBool::new(false));
    let mut dict_cancel: Option<tokio::sync::oneshot::Sender<()>> = None;
    let mut polish_cancel: Option<tokio::sync::oneshot::Sender<()>> = None;

    loop {
        tokio::select! {
            _ = &mut stop_rx => {
                let _ = session.close().await;
                tracing::info!("GlobalShortcuts portal session closed (re-registration)");
                return Ok(());
            }
            ev = activated.next() => {
                let Some(ev) = ev else { break };
                match ev.shortcut_id() {
                    ID_DICTATION => {
                        if dict_active.swap(true, Ordering::SeqCst) {
                            // Missing-release compositor: second press = stop.
                            if let Some(c) = dict_cancel.take() { let _ = c.send(()); }
                            dict_active.store(false, Ordering::SeqCst);
                            let _ = tx.send(HotkeyEvent::DictationReleased);
                        } else {
                            let _ = tx.send(HotkeyEvent::DictationPressed);
                            dict_cancel = Some(arm_release_watchdog(
                                tx.clone(),
                                HotkeyEvent::DictationReleased,
                                dict_active.clone(),
                            ));
                        }
                    }
                    ID_POLISH => {
                        if polish_active.swap(true, Ordering::SeqCst) {
                            if let Some(c) = polish_cancel.take() { let _ = c.send(()); }
                            polish_active.store(false, Ordering::SeqCst);
                            let _ = tx.send(HotkeyEvent::PolishDictationReleased);
                        } else {
                            let _ = tx.send(HotkeyEvent::PolishDictationPressed);
                            polish_cancel = Some(arm_release_watchdog(
                                tx.clone(),
                                HotkeyEvent::PolishDictationReleased,
                                polish_active.clone(),
                            ));
                        }
                    }
                    other => tracing::debug!("portal activated unknown id: {other}"),
                }
            }
            ev = deactivated.next() => {
                let Some(ev) = ev else { break };
                match ev.shortcut_id() {
                    ID_DICTATION => {
                        if let Some(c) = dict_cancel.take() { let _ = c.send(()); }
                        dict_active.store(false, Ordering::SeqCst);
                        let _ = tx.send(HotkeyEvent::DictationReleased);
                    }
                    ID_POLISH => {
                        if let Some(c) = polish_cancel.take() { let _ = c.send(()); }
                        polish_active.store(false, Ordering::SeqCst);
                        let _ = tx.send(HotkeyEvent::PolishDictationReleased);
                    }
                    _ => {}
                }
            }
        }
    }
    // Stream ended (portal restarted / DBus dropped). Surface it — the
    // hotkey is dead until re-registration.
    crate::linux_env::emit_hotkey_status(
        "none",
        "Lost the connection to the desktop's shortcut portal. \
         Re-save your hotkey in Settings to reconnect."
            .to_string(),
    );
    Ok(())
}
