//! RemoteDesktop-portal paste for Wayland.
//!
//! The GlobalShortcuts portal gives us hotkeys; this gives us the other
//! half — injecting the Ctrl+V (or Ctrl+C) keystroke that lands our
//! transcribed text into the focused app, natively, with no wtype /
//! ydotool install. The compositor shows a one-time "allow remote
//! control" dialog; we persist the returned restore_token so it never
//! asks again.
//!
//! The portal is async and its session must stay alive, so it lives in
//! a dedicated actor task: `spawn()` starts it eagerly at launch (the
//! permission dialog lands at boot, not mid-dictation) and the
//! synchronous inject path talks to it over a channel.
//!
//! Self-healing: if session init fails (dialog declined, portal absent,
//! D-Bus hiccup) the actor doesn't die — it parks, and the next paste
//! request triggers a fresh init attempt (10s cooldown so a decline
//! can't turn into dialog spam). A keystroke failure mid-session closes
//! the session and goes back to the same park-and-retry state. Callers
//! always get a reply — Ok, or an Err that tells them to use the
//! wtype/ydotool/X11 fallback chain.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};

// evdev keycodes (Linux input-event-codes) — the space the RemoteDesktop
// portal's notify_keyboard_keycode expects, same as ydotool uses.
pub const EV_V: i32 = 47;
pub const EV_C: i32 = 46;
const EV_LEFTCTRL: i32 = 29;

/// Min gap between portal session-init attempts. A declined permission
/// dialog shouldn't reappear on every dictation in quick succession.
const REINIT_COOLDOWN: Duration = Duration::from_secs(10);

struct PasteReq {
    key: i32, // EV_V or EV_C — pressed together with Left Ctrl
    reply: std_mpsc::Sender<Result<()>>,
}

static TX: OnceLock<tokio::sync::mpsc::UnboundedSender<PasteReq>> = OnceLock::new();
static READY: AtomicBool = AtomicBool::new(false);

/// True once the portal session is live. Informational (status/banner);
/// `send_combo` no longer gates on it — sending while not-ready is what
/// triggers a re-init attempt.
pub fn is_ready() -> bool {
    READY.load(Ordering::Relaxed)
}

/// Send Ctrl+<key> through the portal. Blocks the calling (sync) thread
/// until the actor replies or a timeout elapses. An Err means "use the
/// fallback chain" — and, as a side effect, may have kicked off a portal
/// re-init so the NEXT dictation can go native again.
pub fn send_combo(key: i32) -> Result<()> {
    let tx = TX.get().ok_or_else(|| anyhow!("paste actor not spawned"))?;
    let (rtx, rrx) = std_mpsc::channel();
    tx.send(PasteReq { key, reply: rtx })
        .map_err(|_| anyhow!("paste actor gone"))?;
    // Steady-state round trip is sub-100ms. The generous timeout covers
    // a re-init attempt that pops the permission dialog: if the user is
    // staring at it, this dictation falls back, and the approval fixes
    // the next one.
    rrx.recv_timeout(Duration::from_secs(5))
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

        // A request that arrived while no session was live; it triggered
        // the current re-init and gets served (or refused) right after.
        let mut pending: Option<PasteReq> = None;
        let mut last_attempt: Option<Instant> = None;

        'reinit: loop {
            // First pass runs eagerly at boot so the permission dialog
            // shows at launch. Afterwards, park until a paste request
            // arrives to justify another attempt.
            if last_attempt.is_some() && pending.is_none() {
                match rx.recv().await {
                    Some(req) => pending = Some(req),
                    None => return,
                }
            }
            if let Some(t) = last_attempt {
                if t.elapsed() < REINIT_COOLDOWN {
                    if let Some(req) = pending.take() {
                        let _ = req
                            .reply
                            .send(Err(anyhow!("portal recently failed — using fallback")));
                    }
                    continue 'reinit;
                }
            }
            last_attempt = Some(Instant::now());

            let token = read_restore_token();
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
                    READY.store(false, Ordering::Relaxed);
                    tracing::warn!("RemoteDesktop paste portal init failed: {e}");
                    crate::linux_env::emit_paste_status(
                        "tools",
                        format!(
                            "The desktop's remote-control portal isn't available ({e}). \
                             Falling back to wtype/ydotool if installed. The next \
                             dictation retries the portal."
                        ),
                    );
                    if let Some(req) = pending.take() {
                        let _ = req.reply.send(Err(anyhow!("portal init failed: {e}")));
                    }
                    continue 'reinit;
                }
            };

            READY.store(true, Ordering::Relaxed);
            crate::linux_env::emit_paste_status(
                "portal",
                "Pasting through the desktop portal — no extra tools needed.".to_string(),
            );
            tracing::info!("RemoteDesktop paste portal ready");

            // Serve the request that triggered this init (if any), then
            // the steady stream.
            loop {
                let req = match pending.take() {
                    Some(r) => r,
                    None => match rx.recv().await {
                        Some(r) => r,
                        None => return,
                    },
                };
                let res = async {
                    rd.notify_keyboard_keycode(
                        &session,
                        EV_LEFTCTRL,
                        KeyState::Pressed,
                        Default::default(),
                    )
                    .await?;
                    rd.notify_keyboard_keycode(&session, req.key, KeyState::Pressed, Default::default())
                        .await?;
                    rd.notify_keyboard_keycode(&session, req.key, KeyState::Released, Default::default())
                        .await?;
                    rd.notify_keyboard_keycode(
                        &session,
                        EV_LEFTCTRL,
                        KeyState::Released,
                        Default::default(),
                    )
                    .await?;
                    Ok::<(), ashpd::Error>(())
                }
                .await;

                match res {
                    Ok(()) => {
                        let _ = req.reply.send(Ok(()));
                    }
                    Err(e) => {
                        tracing::warn!("portal keystroke failed: {e}");
                        READY.store(false, Ordering::Relaxed);
                        crate::linux_env::emit_paste_status(
                            "tools",
                            "Lost the remote-control grant — falling back to \
                             wtype/ydotool if installed. The next dictation \
                             retries the portal."
                                .to_string(),
                        );
                        let _ = req.reply.send(Err(anyhow!("portal keystroke: {e}")));
                        let _ = session.close().await;
                        continue 'reinit;
                    }
                }
            }
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
