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

/// Last (backend, detail) reported. Kept because the first status
/// usually fires during Rust setup, before the webview has registered
/// its event listener — `support_info` folds it in so the frontend's
/// initial fetch can't miss it.
static LAST_STATUS: Mutex<Option<(String, String)>> = Mutex::new(None);

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
    serde_json::json!({
        "wayland": wayland,
        "x11_available": has_x11(),
        "desktop": desktop(),
        "gnome": is_gnome(),
        "wtype": which("wtype"),
        "ydotool": which("ydotool"),
        "wl_clipboard": which("wl-copy") && which("wl-paste"),
        "toggle_command": toggle_command,
        "hotkey_backend": hotkey_backend,
        "hotkey_detail": hotkey_detail,
    })
}
