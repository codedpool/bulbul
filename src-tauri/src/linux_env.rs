//! Linux session introspection + support-status plumbing.
//!
//! One place that answers "what kind of Linux is this?" so the hotkey,
//! inject, and overlay code stop making their own guesses:
//!
//!   - Wayland vs X11 session (honoring the BULBUL_FORCE_X11 escape
//!     hatch that the hotkey watcher already documented)
//!   - desktop environment (GNOME needs special-casing for tray icons
//!     and keystroke tools)
//!   - which injection CLI tools are installed
//!
//! Also owns the `linux-hotkey-status` event: whichever backend ends up
//! watching the dictation hotkey (portal, X11 poller, or nothing)
//! reports here, and the frontend banner renders the result. That
//! replaces the old failure mode where a pure-Wayland session logged
//! one tracing::warn and the hotkey silently never fired.

use parking_lot::Mutex;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use tauri::{AppHandle, Emitter};

/// Stashed at setup so status events can be emitted from anywhere on
/// the Linux side (the hotkey watchers don't carry an AppHandle — the
/// cross-platform native watcher signature stays untouched).
static APP_HANDLE: OnceLock<AppHandle> = OnceLock::new();

/// Last hotkey (backend, detail) reported. Kept because the first status
/// usually fires during Rust setup, before the webview has registered
/// its event listener — `support_info` folds it in so the frontend's
/// initial fetch can't miss it.
static LAST_STATUS: Mutex<Option<(String, String)>> = Mutex::new(None);

/// Same idea for the paste backend (portal / tools / none). Lets the
/// support banner suppress the "install ydotool" hint once the
/// RemoteDesktop portal is confirmed working.
static LAST_PASTE_STATUS: Mutex<Option<(String, String)>> = Mutex::new(None);

/// Register Bulbul's app id with xdg-desktop-portal exactly once per
/// process. Without this, GNOME's portal rejects GlobalShortcuts +
/// RemoteDesktop requests with "an app id is required" (the error the
/// Wayland tester hit). Best-effort: pre-1.17 portals lack the Registry
/// interface and error here — harmless, the portal calls then fail the
/// same way they would have anyway. Shared by the hotkey portal and the
/// paste actor; the OnceCell guarantees the underlying D-Bus call runs
/// once even when both race at startup.
pub async fn ensure_host_app_registered() {
    static ONCE: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();
    ONCE.get_or_init(|| async {
        match ashpd::AppID::try_from("com.bulbul.app") {
            Ok(id) => {
                if let Err(e) = ashpd::register_host_app(id).await {
                    tracing::debug!("register_host_app failed (old portal?): {e}");
                }
            }
            Err(e) => tracing::debug!("invalid app id: {e}"),
        }
    })
    .await;
}

pub fn set_app_handle(handle: AppHandle) {
    let _ = APP_HANDLE.set(handle);
}

/// True when running inside a Wayland session. BULBUL_FORCE_X11=1
/// forces the X11/XWayland paths everywhere for debugging.
pub fn is_wayland() -> bool {
    if std::env::var_os("BULBUL_FORCE_X11").is_some_and(|v| v == "1") {
        return false;
    }
    std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var("XDG_SESSION_TYPE").is_ok_and(|v| v.eq_ignore_ascii_case("wayland"))
}

pub fn has_x11() -> bool {
    std::env::var_os("DISPLAY").is_some()
}

pub fn desktop() -> String {
    std::env::var("XDG_CURRENT_DESKTOP")
        .unwrap_or_default()
        .to_lowercase()
}

pub fn is_gnome() -> bool {
    desktop().contains("gnome")
}

/// Cheap `which` — true if the binary is on PATH and executable.
pub fn which(cmd: &str) -> bool {
    Command::new("which")
        .arg(cmd)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// ydotool is only usable when its daemon is running — the client
/// hard-errors without the ydotoold socket. Checks the documented
/// socket locations (env override, per-user runtime dir, system-daemon
/// default).
pub fn ydotool_ready() -> bool {
    if !which("ydotool") {
        return false;
    }
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Some(s) = std::env::var_os("YDOTOOL_SOCKET") {
        candidates.push(s.into());
    }
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        candidates.push(std::path::PathBuf::from(dir).join(".ydotool_socket"));
    }
    candidates.push("/tmp/.ydotool_socket".into());
    candidates.iter().any(|p| p.exists())
}

/// Report which backend ended up owning the dictation hotkey. The
/// frontend banner listens for this; "none" means the user has to act
/// (bind a DE shortcut to the CLI toggle) and the detail says so.
pub fn emit_hotkey_status(backend: &str, detail: String) {
    tracing::info!("linux hotkey status: backend={backend} detail={detail}");
    *LAST_STATUS.lock() = Some((backend.to_string(), detail.clone()));
    if let Some(app) = APP_HANDLE.get() {
        let _ = app.emit(
            "linux-hotkey-status",
            serde_json::json!({ "backend": backend, "detail": detail }),
        );
    }
}

/// Report the paste backend. "portal" = RemoteDesktop portal is live
/// (no tool needed); "tools" = falling back to wtype/ydotool; "none" =
/// nothing works yet.
pub fn emit_paste_status(backend: &str, detail: String) {
    tracing::info!("linux paste status: backend={backend} detail={detail}");
    *LAST_PASTE_STATUS.lock() = Some((backend.to_string(), detail.clone()));
    if let Some(app) = APP_HANDLE.get() {
        let _ = app.emit(
            "linux-paste-status",
            serde_json::json!({ "backend": backend, "detail": detail }),
        );
    }
}

/// Everything the frontend Linux-support banner needs in one call.
/// Command-invoked from the dashboard on Linux only.
pub fn support_info() -> serde_json::Value {
    let wayland = is_wayland();
    let toggle_command = std::env::current_exe()
        .map(|p| format!("{} --toggle-dictation", p.display()))
        .unwrap_or_else(|_| "bulbul --toggle-dictation".into());
    let (hotkey_backend, hotkey_detail) = LAST_STATUS
        .lock()
        .clone()
        .unwrap_or(("unknown".to_string(), String::new()));
    let (paste_backend, paste_detail) = LAST_PASTE_STATUS
        .lock()
        .clone()
        .unwrap_or(("unknown".to_string(), String::new()));
    let gnome = is_gnome();
    serde_json::json!({
        "wayland": wayland,
        "x11_available": has_x11(),
        "desktop": desktop(),
        "gnome": gnome,
        "wtype": which("wtype"),
        // wtype can't type on Mutter even when installed.
        "wtype_usable": which("wtype") && !gnome,
        "ydotool": which("ydotool"),
        // Installed-but-daemonless ydotool can't type either.
        "ydotool_ready": ydotool_ready(),
        "wl_clipboard": which("wl-copy") && which("wl-paste"),
        "toggle_command": toggle_command,
        "hotkey_backend": hotkey_backend,
        "hotkey_detail": hotkey_detail,
        "paste_backend": paste_backend,
        "paste_detail": paste_detail,
    })
}
