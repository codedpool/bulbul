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
                    ID_DICTATION => { let _ = tx.send(HotkeyEvent::DictationPressed); }
                    ID_POLISH => { let _ = tx.send(HotkeyEvent::PolishDictationPressed); }
                    other => tracing::debug!("portal activated unknown id: {other}"),
                }
            }
            ev = deactivated.next() => {
                let Some(ev) = ev else { break };
                match ev.shortcut_id() {
                    ID_DICTATION => { let _ = tx.send(HotkeyEvent::DictationReleased); }
                    ID_POLISH => { let _ = tx.send(HotkeyEvent::PolishDictationReleased); }
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
