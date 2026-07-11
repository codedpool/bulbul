//! Portal-native paste keystrokes for Wayland — no external tools.
//!
//! Two backends, chosen by desktop at spawn():
//!
//! **GNOME → enigo/EIS.** Mutter accepts the RemoteDesktop portal's
//! legacy `NotifyKeyboardKeycode` D-Bus calls and returns success while
//! injecting NOTHING — GNOME only delivers input over the newer EIS
//! (libei) transport. enigo's libei backend does the full EIS handshake
//! (its own RemoteDesktop session + ConnectToEIS via reis). Trade-off:
//! enigo can't persist the permission grant yet (restore token is a
//! TODO upstream), so GNOME shows one "remote control" approval per
//! app launch. Annoying but working beats silent no-op.
//!
//! **Everywhere else (KDE, wlroots) → Notify actor.** KDE implements
//! the legacy Notify* methods properly, and our own ashpd session
//! persists its restore token, so the approval dialog appears once
//! ever. Session lives in a tokio actor task.
//!
//! Both backends are self-healing: failed init parks the backend, and
//! each later paste request retries (10s cooldown so a declined dialog
//! can't turn into dialog spam). Callers always get a reply — Ok, or an
//! Err that sends them down the wtype/ydotool/XWayland fallback chain.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};

// evdev keycodes (Linux input-event-codes) — what the RemoteDesktop
// portal's notify_keyboard_keycode expects, same space ydotool uses.
pub const EV_V: i32 = 47;
pub const EV_C: i32 = 46;
const EV_LEFTCTRL: i32 = 29;

/// Min gap between portal session-init attempts. A declined permission
/// dialog shouldn't reappear on every dictation in quick succession.
const REINIT_COOLDOWN: Duration = Duration::from_secs(10);

/// Requests older than this are dropped unserved — the caller's
/// send_combo timed out long ago and already took the fallback path, so
/// executing the keystroke late would fire a phantom Ctrl+V into
/// whatever the user is doing by then.
const STALE_REQ: Duration = Duration::from_secs(5);

struct PasteReq {
    key: i32, // EV_V or EV_C — pressed together with Left Ctrl
    queued_at: Instant,
    reply: std_mpsc::Sender<Result<()>>,
}

/// Exactly one of these is populated by spawn().
static NOTIFY_TX: OnceLock<tokio::sync::mpsc::UnboundedSender<PasteReq>> = OnceLock::new();
static ENIGO_TX: OnceLock<std_mpsc::Sender<PasteReq>> = OnceLock::new();
static READY: AtomicBool = AtomicBool::new(false);

/// True once a portal/EIS session is live. Informational (status);
/// `send_combo` doesn't gate on it — asking while down is what triggers
/// the self-healing re-init.
pub fn is_ready() -> bool {
    READY.load(Ordering::Relaxed)
}

/// Send Ctrl+<key> through the active backend. Blocks the calling
/// (sync) thread until the backend replies or a timeout elapses. An Err
/// means "use the fallback chain".
pub fn send_combo(key: i32) -> Result<()> {
    let (rtx, rrx) = std_mpsc::channel();
    let req = PasteReq {
        key,
        queued_at: Instant::now(),
        reply: rtx,
    };
    if let Some(tx) = ENIGO_TX.get() {
        tx.send(req).map_err(|_| anyhow!("paste backend gone"))?;
    } else if let Some(tx) = NOTIFY_TX.get() {
        tx.send(req).map_err(|_| anyhow!("paste backend gone"))?;
    } else {
        return Err(anyhow!("paste backend not spawned"));
    }
    // Steady-state round trip is fast; the generous timeout covers a
    // re-init attempt that pops the permission dialog. If the user is
    // staring at it, this dictation falls back and the approval fixes
    // the next one.
    rrx.recv_timeout(Duration::from_secs(5))
        .map_err(|_| anyhow!("portal paste timed out"))?
}

/// Spawn the paste backend for this desktop. Wayland only; call once at
/// setup. No-op if already spawned.
pub fn spawn() {
    if crate::linux_env::is_gnome() {
        spawn_enigo_backend();
    } else {
        spawn_notify_backend();
    }
}

// === GNOME backend: enigo → EIS ============================================

fn spawn_enigo_backend() {
    let (tx, rx) = std_mpsc::channel::<PasteReq>();
    if ENIGO_TX.set(tx).is_err() {
        return;
    }

    // Dedicated OS thread: enigo's libei backend spins its own
    // current-thread tokio runtime internally (custom_block_on), which
    // must not run inside tauri's async runtime.
    std::thread::Builder::new()
        .name("bulbul-eis-paste".into())
        .spawn(move || {
            use enigo::{
                Direction::{Press, Release},
                Enigo, Key, Keyboard, Settings,
            };

            let mut enigo: Option<Enigo> = None;
            let mut last_attempt: Option<Instant> = None;
            let mut pending: Option<PasteReq> = None;

            loop {
                // Eager first init at boot (permission dialog lands at
                // launch); afterwards, park until a request justifies a
                // retry.
                if last_attempt.is_some() && enigo.is_none() && pending.is_none() {
                    match rx.recv() {
                        Ok(req) => pending = Some(req),
                        Err(_) => return,
                    }
                }

                if enigo.is_none() {
                    if let Some(t) = last_attempt {
                        if t.elapsed() < REINIT_COOLDOWN {
                            if let Some(req) = pending.take() {
                                let _ = req
                                    .reply
                                    .send(Err(anyhow!("EIS recently failed — using fallback")));
                            }
                            continue;
                        }
                    }
                    last_attempt = Some(Instant::now());
                    match Enigo::new(&Settings::default()) {
                        Ok(e) => {
                            enigo = Some(e);
                            READY.store(true, Ordering::Relaxed);
                            crate::linux_env::emit_paste_status(
                                "portal",
                                "Typing through the desktop's input portal (EIS) — \
                                 no extra tools needed."
                                    .to_string(),
                            );
                            tracing::info!("enigo EIS paste backend ready");
                        }
                        Err(e) => {
                            READY.store(false, Ordering::Relaxed);
                            tracing::warn!("enigo EIS init failed: {e}");
                            crate::linux_env::emit_paste_status(
                                "tools",
                                format!(
                                    "The desktop's input portal isn't available ({e}). \
                                     Falling back to ydotool if installed. The next \
                                     dictation retries the portal."
                                ),
                            );
                            if let Some(req) = pending.take() {
                                let _ = req.reply.send(Err(anyhow!("EIS init failed: {e}")));
                            }
                            continue;
                        }
                    }
                }

                let req = match pending.take() {
                    Some(r) => r,
                    None => match rx.recv() {
                        Ok(r) => r,
                        Err(_) => return,
                    },
                };
                if req.queued_at.elapsed() > STALE_REQ {
                    continue; // caller gave up long ago — don't fire a phantom paste
                }

                let key_char = if req.key == EV_C { 'c' } else { 'v' };
                let e = enigo.as_mut().expect("enigo present in serve state");
                let res = (|| -> Result<()> {
                    e.key(Key::Control, Press).map_err(|e| anyhow!("ctrl press: {e}"))?;
                    e.key(Key::Unicode(key_char), enigo::Direction::Click)
                        .map_err(|e| anyhow!("key click: {e}"))?;
                    e.key(Key::Control, Release)
                        .map_err(|e| anyhow!("ctrl release: {e}"))?;
                    Ok(())
                })();

                match res {
                    Ok(()) => {
                        let _ = req.reply.send(Ok(()));
                    }
                    Err(err) => {
                        tracing::warn!("EIS keystroke failed: {err:#}");
                        READY.store(false, Ordering::Relaxed);
                        crate::linux_env::emit_paste_status(
                            "tools",
                            "Lost the input-portal connection — falling back to \
                             ydotool if installed. The next dictation retries."
                                .to_string(),
                        );
                        let _ = req.reply.send(Err(err));
                        enigo = None; // drop closes the EIS session; re-init on demand
                    }
                }
            }
        })
        .expect("spawn EIS paste thread");
}

// === Non-GNOME backend: RemoteDesktop Notify* ==============================

fn spawn_notify_backend() {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<PasteReq>();
    if NOTIFY_TX.set(tx).is_err() {
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
                if req.queued_at.elapsed() > STALE_REQ {
                    continue;
                }
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
