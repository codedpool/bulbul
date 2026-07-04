//! RemoteDesktop-portal paste for Wayland.
//!
//! The GlobalShortcuts portal gives us hotkeys; this gives us the other
//! half — injecting the Ctrl+V (or Ctrl+C) keystroke that lands our
//! transcribed text into the focused app, natively, with no wtype /
//! ydotool install. The compositor shows a one-time "allow remote
//! control" dialog; we persist the returned restore_token so it never
//! asks again.
//!
//! The portal is async and its session must stay alive for the app's
//! lifetime, so it lives in a dedicated actor task: `spawn()` starts it
//! (eagerly, at launch, so the permission dialog appears then rather
//! than mid-dictation), and the synchronous inject path talks to it over
//! a channel. Everything is best-effort — if the portal is unavailable,
//! denied, or times out, `is_ready()` stays false / `send_combo` errors
//! and the caller falls back to wtype/ydotool/X11.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{anyhow, Result};

// evdev keycodes (Linux input-event-codes) — the space the RemoteDesktop
// portal's notify_keyboard_keycode expects, same as ydotool uses.
pub const EV_V: i32 = 47;
pub const EV_C: i32 = 46;
const EV_LEFTCTRL: i32 = 29;

struct PasteReq {
    key: i32, // EV_V or EV_C — pressed together with Left Ctrl
    reply: std_mpsc::Sender<Result<()>>,
}

static TX: OnceLock<tokio::sync::mpsc::UnboundedSender<PasteReq>> = OnceLock::new();
static READY: AtomicBool = AtomicBool::new(false);

/// True once the portal session is started and keystrokes can flow.
pub fn is_ready() -> bool {
    READY.load(Ordering::Relaxed)
}

/// Send Ctrl+<key> through the portal. Blocks the calling (sync) thread
/// until the actor replies or a short timeout elapses. Errors if the
/// portal isn't ready or the round-trip fails — caller should fall back.
pub fn send_combo(key: i32) -> Result<()> {
    if !is_ready() {
        return Err(anyhow!("RemoteDesktop portal not ready"));
    }
    let tx = TX.get().ok_or_else(|| anyhow!("paste actor not spawned"))?;
    let (rtx, rrx) = std_mpsc::channel();
    tx.send(PasteReq { key, reply: rtx })
        .map_err(|_| anyhow!("paste actor gone"))?;
    // The keystroke round-trip is sub-100ms once the session is live;
    // 3s is pure safety margin. (The one-time permission dialog is
    // handled at spawn(), never here — READY gates this call.)
    rrx.recv_timeout(Duration::from_secs(3))
        .map_err(|_| anyhow!("portal paste timed out"))?
}

/// Spawn the actor and eagerly initialize the RemoteDesktop session.
/// Wayland only; call once at setup. No-op if already spawned.
pub fn spawn() {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<PasteReq>();
    if TX.set(tx).is_err() {
        return;
    }

    tauri::async_runtime::spawn(async move {
        use ashpd::desktop::remote_desktop::{
            DeviceType, KeyState, RemoteDesktop, SelectDevicesOptions,
        };
        use ashpd::desktop::PersistMode;
        use enumflags2::BitFlags;

        crate::linux_env::ensure_host_app_registered().await;

        // Bound the restore token in a `let` so its String outlives the
        // borrow handed to set_restore_token.
        let token = read_restore_token();

        // Inline init so we never name Session's exact type (it carries a
        // lifetime awkward to spell in a return position). Yields the
        // live (portal, session) pair on success.
        let init = async {
            let rd = RemoteDesktop::new().await?;
            let session = rd.create_session(Default::default()).await?;
            let devices: BitFlags<DeviceType> = DeviceType::Keyboard.into();
            rd.select_devices(
                &session,
                SelectDevicesOptions::default()
                    .set_devices(devices)
                    .set_persist_mode(PersistMode::ExplicitlyRevoked)
                    .set_restore_token(token.as_deref()),
            )
            .await?;
            let response = rd
                .start(&session, None, Default::default())
                .await?
                .response()?;
            if let Some(tok) = response.restore_token() {
                write_restore_token(tok);
            }
            Ok::<_, ashpd::Error>((rd, session))
        };

        let (rd, session) = match init.await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("RemoteDesktop paste portal unavailable: {e}");
                crate::linux_env::emit_paste_status(
                    "tools",
                    "The desktop's RemoteDesktop portal is unavailable — \
                     falling back to wtype/ydotool if installed."
                        .to_string(),
                );
                return;
            }
        };

        READY.store(true, Ordering::Relaxed);
        crate::linux_env::emit_paste_status(
            "portal",
            "Pasting through the desktop portal — no extra tools needed.".to_string(),
        );
        tracing::info!("RemoteDesktop paste portal ready");

        while let Some(req) = rx.recv().await {
            let res = async {
                rd.notify_keyboard_keycode(&session, EV_LEFTCTRL, KeyState::Pressed, Default::default())
                    .await?;
                rd.notify_keyboard_keycode(&session, req.key, KeyState::Pressed, Default::default())
                    .await?;
                rd.notify_keyboard_keycode(&session, req.key, KeyState::Released, Default::default())
                    .await?;
                rd.notify_keyboard_keycode(&session, EV_LEFTCTRL, KeyState::Released, Default::default())
                    .await?;
                Ok::<(), ashpd::Error>(())
            }
            .await;
            if let Err(e) = &res {
                tracing::warn!("portal keystroke failed: {e}");
                // A hard failure mid-session (compositor revoked the
                // grant, D-Bus dropped) means the portal can't be
                // trusted anymore — drop back to the tool path for the
                // rest of the session rather than silently no-op'ing.
                READY.store(false, Ordering::Relaxed);
                crate::linux_env::emit_paste_status(
                    "tools",
                    "Lost the RemoteDesktop portal grant — falling back to \
                     wtype/ydotool if installed."
                        .to_string(),
                );
                let _ = req.reply.send(res.map_err(|e| anyhow!("portal keystroke: {e}")));
                break;
            }
            let _ = req.reply.send(Ok(()));
        }
    });
}

fn token_file() -> Option<std::path::PathBuf> {
    crate::config::config_dir()
        .ok()
        .map(|d| d.join("remote_desktop_token"))
}

fn read_restore_token() -> Option<String> {
    let s = std::fs::read_to_string(token_file()?).ok()?;
    let s = s.trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn write_restore_token(tok: &str) {
    if let Some(p) = token_file() {
        if let Err(e) = std::fs::write(&p, tok) {
            tracing::warn!("could not persist RemoteDesktop restore token: {e}");
        }
    }
}
