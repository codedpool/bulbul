mod audio;
mod config;
mod correction;
mod db;
mod groq;
mod hotkey;
mod inject;
mod telemetry;
mod uia;
mod window_info;

use crate::audio::Recorder;
use crate::config::{CleanupMode, Config};
use crate::hotkey::{HotkeyEvent, HotkeySet, ParsedHotkey};
use parking_lot::Mutex;
use serde::Serialize;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use tauri::{
    image::Image,
    menu::{Menu, MenuItem},
    tray::{TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, LogicalPosition, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent,
};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_notification::NotificationExt;

const OVERLAY_WIDTH: f64 = 240.0;
const OVERLAY_HEIGHT: f64 = 48.0;
// Gap between the pill and the top of the taskbar / work area.
const OVERLAY_BOTTOM_MARGIN: f64 = 4.0;

pub struct AppState {
    config: Arc<Mutex<Config>>,
    hotkeys: Arc<Mutex<HotkeySet>>,
    /// Sender feeding the orchestrator. Stored so that re-registering
    /// shortcuts after a settings change reuses the same channel.
    hotkey_tx: std::sync::mpsc::Sender<HotkeyEvent>,
    /// Status of each transform slot hotkey (filled after every
    /// re-registration). The Transforms UI reads this to label cards.
    transform_slot_statuses: Arc<Mutex<Vec<hotkey::TransformSlotStatus>>>,
    icons: Arc<IconVariants>,
    db: db::Db,
    /// Cached compiled dictionary + snippet regexes. Saves ~50ms per
    /// dictation vs. recompiling from the DB row each time. Invalidated
    /// whenever a CRUD command on those tables runs.
    regex_cache: Arc<db::RegexCache>,
    /// Mode-B auto-update slot. The background watcher (see
    /// `spawn_update_watcher`) downloads new installers into here on
    /// discovery. The frontend banner reads `version` to render, and the
    /// "Install & restart" button consumes the `Update` to run the
    /// installer. When `Some`, an update is sitting on disk waiting to
    /// be applied — the user picks the moment.
    staged_update: Arc<Mutex<Option<StagedUpdate>>>,
}

/// A downloaded-but-not-yet-installed update. Holds the Tauri `Update`
/// handle (so we can call `install` later) alongside the already-downloaded
/// installer bytes and a stable version string for the UI.
pub struct StagedUpdate {
    update: tauri_plugin_updater::Update,
    bytes: Vec<u8>,
    pub version: String,
}

struct PendingDictation {
    started_at: Instant,
    foreground_app: Option<String>,
    language: String,
    mode: CleanupMode,
}

struct IconVariants {
    active: OwnedIcon,
    no_key: OwnedIcon,
}

#[derive(Clone)]
struct OwnedIcon {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
}

impl OwnedIcon {
    fn from_image(img: &Image<'_>) -> Self {
        Self {
            rgba: img.rgba().to_vec(),
            width: img.width(),
            height: img.height(),
        }
    }
    fn to_image(&self) -> Image<'static> {
        Image::new_owned(self.rgba.clone(), self.width, self.height)
    }
    fn tinted_red(&self) -> Self {
        let mut rgba = self.rgba.clone();
        for chunk in rgba.chunks_exact_mut(4) {
            if chunk[3] == 0 {
                continue;
            }
            // Pull each opaque pixel toward a muted red.
            chunk[0] = (((chunk[0] as u32) + 220 * 2) / 3) as u8;
            chunk[1] = ((chunk[1] as u32) / 3) as u8;
            chunk[2] = ((chunk[2] as u32) / 3) as u8;
        }
        Self {
            rgba,
            width: self.width,
            height: self.height,
        }
    }
}

#[derive(Clone, Serialize)]
struct StatusPayload {
    state: &'static str,
    message: Option<String>,
}

fn emit_status(app: &AppHandle, state: &'static str, message: Option<String>) {
    tracing::debug!("emit_status: state={state}");
    let _ = app.emit(
        "bulbul-status",
        StatusPayload {
            state,
            message: message.clone(),
        },
    );
    // Respect the user's "hide tray icon" mode: when on, the overlay
    // window is hidden during idle and shown only while a dictation is
    // in flight. When off (default), the overlay stays visible across
    // every state.
    apply_overlay_visibility_for_state(app, state);
    // Force the overlay above Bulbul's own main window when an active state
    // begins. `always_on_top: true` was set at window creation but Windows
    // doesn't reliably honour that between same-process windows — we have
    // to call SetWindowPos(HWND_TOPMOST, SWP_NOACTIVATE) ourselves, and
    // dispatch it to the UI thread (some Win32 calls are flaky cross-thread).
    if matches!(state, "listening" | "processing" | "injecting") {
        let app_for_top = app.clone();
        let _ = app.run_on_main_thread(move || {
            if let Some(overlay) = app_for_top.get_webview_window("overlay") {
                bring_overlay_to_top(&overlay);
            } else {
                tracing::warn!("emit_status: no overlay window found for state={state}");
            }
        });
    }
    // After a terminal state, fall back to idle so the overlay shrinks.
    // Re-enter emit_status (not just app.emit) so the visibility logic
    // fires on the idle transition too.
    if matches!(state, "done" | "error") {
        let app_clone = app.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_millis(1100)).await;
            emit_status(&app_clone, "idle", None);
        });
    }
}

/// Show or hide the overlay window based on the current dictation state
/// and the user's `hide_tray` preference. When `hide_tray` is on, the
/// overlay is hidden during `idle` so the screen is clean, and shown for
/// every active state so the user always gets visual feedback while
/// dictating. When `hide_tray` is off, the overlay stays visible at all
/// times (idle just shows the small pill).
fn apply_overlay_visibility_for_state(app: &AppHandle, state: &str) {
    let hide_tray = app.state::<AppState>().config.lock().hide_tray;
    let Some(overlay) = app.get_webview_window("overlay") else { return; };
    let should_show = !hide_tray || state != "idle";
    let result = if should_show { overlay.show() } else { overlay.hide() };
    if let Err(e) = result {
        tracing::warn!("overlay visibility toggle failed (state={state}): {e}");
    }
}

/// Pull the overlay to the very top of the system z-order without taking
/// focus. We do this via raw FFI to avoid a HWND type mismatch between
/// Tauri's bundled `windows` crate version and our own.
fn bring_overlay_to_top(window: &tauri::WebviewWindow) {
    #[cfg(target_os = "windows")]
    {
        use std::ffi::c_void;
        type HwndPtr = *mut c_void;
        const HWND_TOPMOST: HwndPtr = -1isize as HwndPtr;
        const SWP_NOMOVE: u32 = 0x0002;
        const SWP_NOSIZE: u32 = 0x0001;
        const SWP_NOACTIVATE: u32 = 0x0010;
        const SWP_SHOWWINDOW: u32 = 0x0040;

        #[link(name = "user32")]
        extern "system" {
            fn SetWindowPos(
                h_wnd: HwndPtr,
                h_wnd_insert_after: HwndPtr,
                x: i32,
                y: i32,
                cx: i32,
                cy: i32,
                u_flags: u32,
            ) -> i32;
        }

        #[link(name = "kernel32")]
        extern "system" {
            fn GetLastError() -> u32;
        }

        let hwnd = match window.hwnd() {
            Ok(h) => h.0 as HwndPtr,
            Err(e) => {
                tracing::warn!("could not get overlay HWND: {e:#}");
                return;
            }
        };
        let result = unsafe {
            SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
            )
        };
        if result == 0 {
            let err = unsafe { GetLastError() };
            tracing::warn!("SetWindowPos(TOPMOST) failed: GetLastError={err}, hwnd={hwnd:?}");
        } else {
            tracing::info!("SetWindowPos(TOPMOST) ok for overlay hwnd={hwnd:?}");
        }
    }
    #[cfg(not(target_os = "windows"))]
    let _ = window;
}

fn position_overlay_bottom_center(app: &AppHandle) {
    let Some(window) = app.get_webview_window("overlay") else {
        return;
    };
    let Ok(Some(monitor)) = window.primary_monitor() else {
        return;
    };
    let scale = monitor.scale_factor();
    let size = monitor.size();
    let logical_w = size.width as f64 / scale;
    let logical_h = size.height as f64 / scale;

    // Anchor to the bottom of the Windows work area (i.e. just above the
    // taskbar) when we can resolve it, otherwise fall back to the full
    // monitor bottom.
    let anchor_bottom = work_area_bottom_logical(scale).unwrap_or(logical_h);

    let x = (logical_w - OVERLAY_WIDTH) / 2.0;
    let y = anchor_bottom - OVERLAY_HEIGHT - OVERLAY_BOTTOM_MARGIN;
    let _ = window.set_position(LogicalPosition::new(x, y));
}

/// Hover-aware click-through: a polling thread that watches the cursor.
/// When the cursor enters a small zone near the pill, click-through is
/// disabled (so satellite buttons can be clicked) and a "hovered" event is
/// emitted to the frontend. Larger exit zone gives hysteresis so the
/// expanded UI doesn't flicker as the cursor moves between buttons.
///
/// Windows-only for now — Phase 7 of the macOS port replaces the Win32
/// GetCursorPos call with NSEvent.mouseLocation. Until then Mac gets the
/// no-op stub below: the pill renders fine, but satellite buttons aren't
/// reachable (cursor-driven expansion is disabled).
#[cfg(target_os = "windows")]
fn spawn_hover_watcher(app: AppHandle) {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

    thread::spawn(move || {
        let mut last_hovered = false;
        loop {
            thread::sleep(Duration::from_millis(50));
            let Some(overlay) = app.get_webview_window("overlay") else {
                continue;
            };
            let Ok(pos) = overlay.outer_position() else { continue; };
            let Ok(size) = overlay.outer_size() else { continue; };

            let mut p = POINT::default();
            if unsafe { GetCursorPos(&mut p) }.is_err() {
                continue;
            }

            let x0 = pos.x;
            let y0 = pos.y;
            let w = size.width as i32;
            let h = size.height as i32;
            let cx = x0 + w / 2;
            let cy = y0 + h - 24; // pill sits near the bottom of the window

            // Entry zone (small, near the dot): triggers expansion.
            let entry_w = 100;
            let entry_h = 40;
            let in_entry = (p.x - cx).abs() < entry_w / 2
                && (p.y - cy).abs() < entry_h / 2;

            // Exit zone (full window): keeps expansion active.
            let in_exit = p.x >= x0 && p.x < x0 + w && p.y >= y0 && p.y < y0 + h;

            let new_hovered = if last_hovered { in_exit } else { in_entry };
            if new_hovered != last_hovered {
                last_hovered = new_hovered;
                let _ = overlay.set_ignore_cursor_events(!new_hovered);
                let _ = app.emit("overlay-hover", new_hovered);
            }
        }
    });
}

#[cfg(not(target_os = "windows"))]
fn spawn_hover_watcher(_app: AppHandle) {
    // Phase 7: implement via NSEvent.mouseLocation. Until then no hover.
}


/// Background loop that polls GitHub Releases for newer Bulbul versions
/// and silently downloads them into the AppState's `staged_update` slot.
/// The frontend listens for the `update-staged` Tauri event and renders
/// a banner; nothing else happens until the user (or the tray Quit) calls
/// `install_staged_update`.
///
/// Cadence:
/// - 10s grace after boot so we don't fight with first-dictation traffic
/// - Re-check every 6 hours while the app stays open
/// - Skip checks while an update is already staged (no double-downloads)
fn spawn_update_watcher(app: AppHandle) {
    use tauri_plugin_updater::UpdaterExt;
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(10)).await;
        loop {
            // Skip if an update is already sitting in the slot — the user
            // hasn't restarted yet and re-downloading is wasted bandwidth.
            // Clone the Arc out of State before locking so the lifetime
            // chain from `app.state()` doesn't trip the borrow checker.
            let staged_arc = app.state::<AppState>().staged_update.clone();
            let already_staged = staged_arc.lock().is_some();
            if !already_staged {
                match app.updater() {
                    Ok(updater) => match updater.check().await {
                        Ok(Some(update)) => {
                            let version = update.version.clone();
                            tracing::info!("update watcher: v{version} available, downloading…");
                            match update.download(|_chunk, _len| {}, || {}).await {
                                Ok(bytes) => {
                                    let slot = app.state::<AppState>().staged_update.clone();
                                    *slot.lock() = Some(StagedUpdate {
                                        update,
                                        bytes,
                                        version: version.clone(),
                                    });
                                    tracing::info!(
                                        "update watcher: v{version} downloaded, staged for install"
                                    );
                                    let _ = app.emit("update-staged", version);
                                }
                                Err(e) => {
                                    tracing::warn!("update download failed: {e:#}");
                                }
                            }
                        }
                        Ok(None) => {
                            tracing::debug!("update watcher: already up to date");
                        }
                        Err(e) => {
                            tracing::debug!("update watcher check failed: {e:#}");
                        }
                    },
                    Err(e) => {
                        tracing::debug!("update watcher: updater not available: {e:#}");
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(6 * 3600)).await;
        }
    });
}

/// If an update is staged, run the installer synchronously before exit.
/// Called from the tray Quit handler — turns "Quit" into "Quit and install".
/// On the happy path the installer kills our process mid-call and the
/// function never returns; on failure we log and let the normal exit
/// continue.
fn install_staged_if_present(app: &AppHandle) {
    let slot = app.state::<AppState>().staged_update.clone();
    let staged = slot.lock().take();
    let Some(staged) = staged else {
        return;
    };
    tracing::info!("tray quit: installing staged update v{}", staged.version);
    // install is sync in Tauri 2's updater plugin — it writes the bytes
    // to a temp file and spawns the installer. We don't await anything.
    if let Err(e) = staged.update.install(staged.bytes) {
        tracing::warn!("staged-update install failed on quit: {e:#}");
    }
}

/// Logical y-coordinate of the bottom of the screen's work area (just
/// above the taskbar/dock). The overlay pill anchors itself there.
///
/// Phase 7 will swap the Mac arm in for NSScreen.visibleFrame. Until
/// then Mac returns None and the caller falls back to the full monitor
/// bottom — the pill sits flush with the screen edge instead of above
/// the dock.
#[cfg(target_os = "windows")]
fn work_area_bottom_logical(scale: f64) -> Option<f64> {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::{
        SystemParametersInfoW, SPI_GETWORKAREA, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
    };
    let mut rect = RECT::default();
    let res = unsafe {
        SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some(&mut rect as *mut _ as *mut std::ffi::c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    res.ok()?;
    Some(rect.bottom as f64 / scale)
}

/// Mac: NSScreen.visibleFrame gives us the work area minus menu bar
/// and dock. We convert from Mac's bottom-left origin to the top-down
/// coords the overlay positioning expects.
///
/// MainThreadMarker::new() returns None when called off the main thread,
/// in which case we fall back to "no work area known" and the caller
/// uses the full monitor bottom (the overlay sits flush with the dock).
/// position_overlay_bottom_center runs from window event handlers which
/// are dispatched on the main thread, so the marker normally resolves.
#[cfg(target_os = "macos")]
fn work_area_bottom_logical(_scale: f64) -> Option<f64> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSScreen;
    let mtm = MainThreadMarker::new()?;
    let screen = NSScreen::mainScreen(mtm)?;
    let frame = screen.frame();
    let visible = screen.visibleFrame();
    // NSScreen geometry is in points (already logical) with origin at
    // the screen's bottom-left. The overlay code expects top-down y.
    //   top-down(work-area-bottom) = frame.height - visible.origin.y
    Some(frame.size.height - visible.origin.y)
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn work_area_bottom_logical(_scale: f64) -> Option<f64> {
    None
}

fn notify(app: &AppHandle, title: &str, body: &str) {
    let _ = app
        .notification()
        .builder()
        .title(title)
        .body(body)
        .show();
}

fn update_tray_icon(app: &AppHandle, has_key: bool) {
    let Some(tray) = app.tray_by_id("bulbul-tray") else {
        return;
    };
    // On Mac the tray icon is a template image set once at build time.
    // tray.set_icon would replace the underlying NSImage and lose the
    // template flag, so we skip the icon swap there and let the
    // tooltip carry the missing-key state instead. Win/Linux still
    // swap to the tinted-red variant.
    #[cfg(not(target_os = "macos"))]
    {
        let state = app.state::<AppState>();
        let icon = if has_key {
            state.icons.active.to_image()
        } else {
            state.icons.no_key.to_image()
        };
        let _ = tray.set_icon(Some(icon));
    }
    let tooltip = if has_key {
        "Bulbul — hold your hotkey to dictate"
    } else {
        "Bulbul — set your Groq API key in Settings"
    };
    let _ = tray.set_tooltip(Some(tooltip));
}

#[tauri::command]
fn get_config(state: tauri::State<'_, AppState>) -> Config {
    state.config.lock().clone()
}

#[tauri::command]
fn save_config(
    new_cfg: Config,
    state: tauri::State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    let (prev_has_key, prev_hotkey, prev_pol, prev_theme, prev_mode, prev_telemetry, prev_style) = {
        let cfg = state.config.lock();
        (
            cfg.has_api_key(),
            cfg.hotkey.clone(),
            cfg.polish_hotkey.clone(),
            cfg.theme.clone(),
            cfg.mode.as_str().to_string(),
            cfg.telemetry_enabled,
            cfg.style_enabled,
        )
    };
    config::save(&new_cfg).map_err(|e| format!("{e:#}"))?;
    let next_has_key = new_cfg.has_api_key();
    let next_hotkey = new_cfg.hotkey.clone();
    let next_pol = new_cfg.polish_hotkey.clone();
    let next_theme = new_cfg.theme.clone();
    let next_mode = new_cfg.mode.as_str().to_string();
    let next_telemetry = new_cfg.telemetry_enabled;
    let next_style = new_cfg.style_enabled;
    *state.config.lock() = new_cfg;

    // Telemetry: emit one event per actual change so dashboards can count
    // which knobs people actually turn. We never send the value (e.g. the
    // specific hotkey string), only the field name. Gated on the *new*
    // value of telemetry_enabled so flipping it ON emits a final
    // confirmation event, and flipping it OFF silently stops sending.
    if next_telemetry {
        if prev_hotkey != next_hotkey {
            telemetry::track("settings_changed", serde_json::json!({"field": "hotkey"}));
        }
        if prev_pol != next_pol {
            telemetry::track("settings_changed", serde_json::json!({"field": "polish_hotkey"}));
        }
        if prev_mode != next_mode {
            telemetry::track("settings_changed", serde_json::json!({"field": "mode", "value": next_mode}));
        }
        if prev_theme != next_theme {
            telemetry::track("settings_changed", serde_json::json!({"field": "theme", "value": next_theme}));
        }
        if prev_style != next_style {
            telemetry::track("settings_changed", serde_json::json!({"field": "style_enabled", "value": next_style}));
        }
        if prev_telemetry != next_telemetry {
            telemetry::track("settings_changed", serde_json::json!({"field": "telemetry_enabled", "value": true}));
        }
    }

    if prev_has_key != next_has_key {
        update_tray_icon(&app, next_has_key);
    }
    if prev_theme != next_theme {
        // Broadcast to every window so the dashboard + scratchpad re-theme live.
        let _ = app.emit("theme-changed", next_theme);
    }
    if prev_hotkey != next_hotkey || prev_pol != next_pol {
        {
            let mut set = state.hotkeys.lock();
            set.dictation = ParsedHotkey::parse(&next_hotkey);
            set.polish_dictation = ParsedHotkey::parse(&next_pol);
        }
        hotkey::install_global_shortcuts(&app, state.hotkeys.clone(), state.hotkey_tx.clone());
    }
    Ok(())
}

/// Recompute the per-transform slot hotkeys (Alt+1..Alt+9 by sort order)
/// and re-register them with the global-shortcut plugin. Call after any
/// transform CRUD operation. Failures (e.g. another app owns the combo)
/// are reported per-slot via AppState.transform_slot_statuses, which the
/// frontend reads to show "unavailable" chips.
fn refresh_transform_bindings(app: &AppHandle, state: &AppState) {
    let transforms = match db::list_transforms(&state.db) {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!("could not list transforms for bindings: {e:#}");
            return;
        }
    };
    let bindings: Vec<(i64, ParsedHotkey)> = transforms
        .iter()
        .take(9)
        .enumerate()
        .map(|(idx, t)| {
            let slot = (idx + 1) as u8;
            let key = ((b'0' + slot) as char).to_string();
            let hk = ParsedHotkey {
                ctrl: false,
                shift: false,
                alt: true,
                meta: false,
                key: Some(key),
            };
            (t.id, hk)
        })
        .collect();
    state.hotkeys.lock().transform_bindings = bindings;

    let statuses = hotkey::install_global_shortcuts(
        app,
        state.hotkeys.clone(),
        state.hotkey_tx.clone(),
    );
    *state.transform_slot_statuses.lock() = statuses;
}

#[tauri::command]
async fn validate_api_key(api_key: String) -> Result<(), String> {
    groq::validate_key(api_key.trim())
        .await
        .map_err(|e| format!("{e:#}"))
}

/// Mark the first-run wizard as finished and persist. Called whether the
/// user completed all the steps or chose "Skip for now" — both should
/// stop the wizard from re-appearing on the next launch.
///
/// Side effect: enables "Start Bulbul with Windows" by default for fresh
/// installs. A dictation app you have to remember to launch every morning
/// is one a friend tries twice and forgets — the value compounds when
/// it's just always there in the tray. The user can flip it off any time
/// in Settings → Startup (or Sidebar → Open at startup), and existing
/// installs that already finished onboarding aren't touched.
#[tauri::command]
fn complete_onboarding(
    state: tauri::State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    let mut cfg = state.config.lock();
    cfg.onboarding_completed = true;
    config::save(&cfg).map_err(|e| format!("{e:#}"))?;
    drop(cfg); // release the lock before the autostart write, in case it blocks
    if let Err(e) = app.autolaunch().enable() {
        // Best-effort. Common reasons it can fail: corporate-locked
        // registry, antivirus interception. We don't fail onboarding for
        // it — the user can always toggle it from Settings.
        tracing::warn!("could not enable autostart on onboarding completion: {e:#}");
    }
    Ok(())
}

/// Frontend-callable telemetry pass-through. The React side uses this for
/// events that originate in the UI (onboarding step completion, etc.). The
/// `telemetry_enabled` gate lives here so the JS caller never has to know
/// or check — flip the master toggle off and every track_event becomes a
/// no-op automatically.
#[tauri::command]
fn track_event(
    event_name: String,
    props: Option<serde_json::Value>,
    state: tauri::State<'_, AppState>,
) {
    if !state.config.lock().telemetry_enabled {
        return;
    }
    let props = props.unwrap_or_else(|| serde_json::json!({}));
    telemetry::track(&event_name, props);
}

#[tauri::command]
fn show_settings_window(app: AppHandle) {
    show_settings(&app);
}

#[tauri::command]
fn get_home_stats(state: tauri::State<'_, AppState>) -> Result<db::HomeStats, String> {
    db::home_stats(&state.db).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
fn get_recent_dictations(
    limit: u32,
    offset: u32,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<db::DictationRow>, String> {
    db::recent_dictations(&state.db, limit, offset).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
fn get_insights_usage(state: tauri::State<'_, AppState>) -> Result<db::UsageStats, String> {
    db::usage_stats(&state.db).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
fn get_voice_stats(state: tauri::State<'_, AppState>) -> Result<db::VoiceStats, String> {
    let has_key = state.config.lock().has_api_key();
    db::voice_stats(&state.db, has_key).map_err(|e| format!("{e:#}"))
}

/// Gather stats + samples, ask Groq for the two narrative blurbs, and persist
/// them. Shared by the manual Refresh command and the automatic refresh that
/// fires after enough new dictations.
async fn regenerate_voice_narrative(cfg: &Config, db: &db::Db) -> anyhow::Result<()> {
    let stats = db::voice_stats(db, true)?;
    let mut summary_lines = Vec::<String>::new();
    summary_lines.push(format!("Total words dictated: {}", stats.total_words));
    if let Some(w) = &stats.most_used_word {
        summary_lines.push(format!("Most used word: {}", w));
    }
    if let Some(w) = &stats.most_corrected_word {
        summary_lines.push(format!("Most corrected word: {}", w));
    }
    if let Some(p) = &stats.catchphrase {
        summary_lines.push(format!("Most repeated phrase: \"{}\"", p));
    }
    if let (Some(d), Some(h)) = (&stats.peak_day_name, &stats.peak_hour_label) {
        summary_lines.push(format!("Peak time: {} at {}", d, h));
    }
    if let Some(app) = &stats.peak_app {
        summary_lines.push(format!("Peak app: {}", app));
    }
    let stats_summary = summary_lines.join("\n");
    let samples = db::voice_profile_context(db)?;
    let (voice_narrative, peak_narrative) =
        groq::generate_voice_profile(cfg, &stats_summary, &samples).await?;
    db::save_voice_narrative(db, &voice_narrative, &peak_narrative)?;
    Ok(())
}

/// Number of new dictations (since the last generation) that triggers an
/// automatic voice-profile refresh. Kept high so the profile only refreshes
/// once a meaningful amount of new material has accrued — it's a nice-to-have
/// that spends a Groq call, not something to regenerate constantly.
const VOICE_AUTO_REFRESH_AFTER: i64 = 100;

/// Fire-and-forget background refresh of the voice profile once enough new
/// dictations have accrued. Only *refreshes* an existing profile — the first
/// generation stays a deliberate manual action so we never spend the user's
/// Groq quota unprompted.
fn maybe_auto_refresh_voice(app: &AppHandle, cfg: &Config, db: &db::Db) {
    if !cfg.has_api_key() {
        return;
    }
    // Cheap timestamp lookup — never the full voice_stats aggregation on the
    // hot path. Only *refresh* an existing profile; first gen stays manual.
    let Some(last) = db::voice_last_generated_at(db).ok().flatten() else {
        return;
    };
    if db::dictations_since(db, last).unwrap_or(0) < VOICE_AUTO_REFRESH_AFTER {
        return;
    }
    let cfg = cfg.clone();
    let db = db.clone();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        match regenerate_voice_narrative(&cfg, &db).await {
            Ok(()) => {
                tracing::info!(
                    "voice profile auto-refreshed ({}+ new dictations)",
                    VOICE_AUTO_REFRESH_AFTER
                );
                let _ = app.emit("voice-profile-updated", ());
            }
            Err(e) => tracing::warn!("voice profile auto-refresh failed: {e:#}"),
        }
    });
}

#[tauri::command]
async fn refresh_voice_narrative(app: AppHandle) -> Result<db::VoiceStats, String> {
    let state = app.state::<AppState>();
    let cfg = state.config.lock().clone();
    let db = state.db.clone();
    if !cfg.has_api_key() {
        return Err("Set your Groq API key in Settings first.".into());
    }
    regenerate_voice_narrative(&cfg, &db)
        .await
        .map_err(|e| format!("{e:#}"))?;
    db::voice_stats(&db, true).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
fn list_dictionary(state: tauri::State<'_, AppState>) -> Result<Vec<db::DictionaryEntry>, String> {
    db::list_dictionary(&state.db).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
fn add_dictionary_entry(
    from_word: String,
    to_word: String,
    case_sensitive: bool,
    state: tauri::State<'_, AppState>,
) -> Result<db::DictionaryEntry, String> {
    let out = db::add_dictionary_entry(&state.db, &from_word, &to_word, case_sensitive)
        .map_err(|e| format!("{e:#}"))?;
    state.regex_cache.invalidate_dictionary();
    Ok(out)
}

#[tauri::command]
fn update_dictionary_entry(
    id: i64,
    from_word: String,
    to_word: String,
    case_sensitive: bool,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    db::update_dictionary_entry(&state.db, id, &from_word, &to_word, case_sensitive)
        .map_err(|e| format!("{e:#}"))?;
    state.regex_cache.invalidate_dictionary();
    Ok(())
}

#[tauri::command]
fn delete_dictionary_entry(
    id: i64,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    db::delete_dictionary_entry(&state.db, id).map_err(|e| format!("{e:#}"))?;
    state.regex_cache.invalidate_dictionary();
    Ok(())
}

#[tauri::command]
fn correction_suggestions(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<db::CorrectionSuggestion>, String> {
    db::correction_suggestions(&state.db).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
fn dismiss_correction_suggestion(
    from_word: String,
    to_word: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    db::dismiss_correction_suggestion(&state.db, &from_word, &to_word)
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
fn list_corrections(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<db::CorrectionHistoryRow>, String> {
    db::list_corrections(&state.db, 100).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
fn list_snippets(state: tauri::State<'_, AppState>) -> Result<Vec<db::Snippet>, String> {
    db::list_snippets(&state.db).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
fn add_snippet(
    trigger: String,
    expansion: String,
    state: tauri::State<'_, AppState>,
) -> Result<db::Snippet, String> {
    let out =
        db::add_snippet(&state.db, &trigger, &expansion).map_err(|e| format!("{e:#}"))?;
    state.regex_cache.invalidate_snippets();
    Ok(out)
}

#[tauri::command]
fn update_snippet(
    id: i64,
    trigger: String,
    expansion: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    db::update_snippet(&state.db, id, &trigger, &expansion).map_err(|e| format!("{e:#}"))?;
    state.regex_cache.invalidate_snippets();
    Ok(())
}

#[tauri::command]
fn delete_snippet(id: i64, state: tauri::State<'_, AppState>) -> Result<(), String> {
    db::delete_snippet(&state.db, id).map_err(|e| format!("{e:#}"))?;
    state.regex_cache.invalidate_snippets();
    Ok(())
}

#[tauri::command]
fn list_transforms(state: tauri::State<'_, AppState>) -> Result<Vec<db::Transform>, String> {
    db::list_transforms(&state.db).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
fn add_transform(
    name: String,
    description: String,
    system_prompt: String,
    state: tauri::State<'_, AppState>,
    app: AppHandle,
) -> Result<db::Transform, String> {
    let out = db::add_transform(&state.db, &name, &description, &system_prompt)
        .map_err(|e| format!("{e:#}"))?;
    refresh_transform_bindings(&app, &state);
    Ok(out)
}

#[tauri::command]
fn update_transform(
    id: i64,
    name: String,
    description: String,
    system_prompt: String,
    state: tauri::State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    db::update_transform(&state.db, id, &name, &description, &system_prompt)
        .map_err(|e| format!("{e:#}"))?;
    refresh_transform_bindings(&app, &state);
    Ok(())
}

#[tauri::command]
fn delete_transform(
    id: i64,
    state: tauri::State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    db::delete_transform(&state.db, id).map_err(|e| format!("{e:#}"))?;
    refresh_transform_bindings(&app, &state);
    Ok(())
}

/// Run a Transform against arbitrary text (used by the Scratchpad to rewrite a
/// selection in place — no clipboard or global hotkey involved). Returns the
/// rewritten text; the caller substitutes it back into the note.
#[tauri::command]
async fn run_transform_on_text(
    transform_id: i64,
    text: String,
    app: AppHandle,
) -> Result<String, String> {
    let (cfg, db) = {
        let state = app.state::<AppState>();
        let cfg = state.config.lock().clone();
        let db = state.db.clone();
        (cfg, db)
    };
    if !cfg.has_api_key() {
        return Err("Set your Groq API key in Settings first.".into());
    }
    if text.trim().is_empty() {
        return Ok(text);
    }
    let transform = db::list_transforms(&db)
        .map_err(|e| format!("{e:#}"))?
        .into_iter()
        .find(|t| t.id == transform_id)
        .ok_or_else(|| "Transform not found".to_string())?;
    let out = groq::execute_transform(&cfg, &transform.system_prompt, &text, None)
        .await
        .map_err(|e| format!("{e:#}"))?;
    let _ = db::bump_transform_hits(&db, transform_id);
    Ok(out)
}

#[tauri::command]
fn set_default_transform(
    id: i64,
    state: tauri::State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    db::set_default_transform(&state.db, id).map_err(|e| format!("{e:#}"))?;
    refresh_transform_bindings(&app, &state);
    Ok(())
}

#[tauri::command]
fn reset_transforms(state: tauri::State<'_, AppState>, app: AppHandle) -> Result<(), String> {
    db::reset_transforms_to_defaults(&state.db).map_err(|e| format!("{e:#}"))?;
    refresh_transform_bindings(&app, &state);
    Ok(())
}

#[tauri::command]
fn list_transform_slot_statuses(
    state: tauri::State<'_, AppState>,
) -> Vec<hotkey::TransformSlotStatus> {
    state.transform_slot_statuses.lock().clone()
}

#[tauri::command]
fn list_notes(state: tauri::State<'_, AppState>) -> Result<Vec<db::Note>, String> {
    db::list_notes(&state.db).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
fn get_note(id: i64, state: tauri::State<'_, AppState>) -> Result<db::Note, String> {
    db::get_note(&state.db, id).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
fn create_note(
    title: String,
    body: String,
    state: tauri::State<'_, AppState>,
    app: AppHandle,
) -> Result<db::Note, String> {
    let note = db::create_note(&state.db, &title, &body).map_err(|e| format!("{e:#}"))?;
    let _ = app.emit("notes-changed", ());
    Ok(note)
}

#[tauri::command]
fn update_note(
    id: i64,
    title: String,
    body: String,
    state: tauri::State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    db::update_note(&state.db, id, &title, &body).map_err(|e| format!("{e:#}"))?;
    let _ = app.emit("notes-changed", ());
    Ok(())
}

#[tauri::command]
fn delete_note(
    id: i64,
    state: tauri::State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    db::delete_note(&state.db, id).map_err(|e| format!("{e:#}"))?;
    let _ = app.emit("notes-changed", ());
    Ok(())
}

#[tauri::command]
fn open_scratchpad(app: AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("scratchpad")
        .ok_or_else(|| "scratchpad window not initialized".to_string())?;
    // Hide-on-close means the window persists; just bring it back.
    let _ = window.show();
    let _ = window.unminimize();
    let _ = window.set_focus();
    Ok(())
}

fn setup_scratchpad_window(app: &AppHandle) -> tauri::Result<()> {
    // Pre-create the scratchpad window at startup, hidden. Creating it lazily
    // on first user click was hanging the WebView (white screen, unresponsive
    // controls) — we suspect a Tauri 2 + WebView2 quirk around lazy window
    // creation in dev mode. Building it during boot gives the WebView2 host
    // time to fully initialize before the user ever interacts with it.
    let window = WebviewWindowBuilder::new(
        app,
        "scratchpad",
        WebviewUrl::App("index.html#scratchpad".into()),
    )
    .title("Bulbul Scratchpad")
    .inner_size(760.0, 540.0)
    .min_inner_size(520.0, 380.0)
    .decorations(false)
    .center()
    .resizable(true)
    .maximizable(false)
    .skip_taskbar(false)
    .visible(false)
    .build()?;

    // Intercept the X button so the window persists across opens.
    let win_handle = window.clone();
    window.on_window_event(move |event| {
        if let WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            let _ = win_handle.hide();
        }
    });
    Ok(())
}

/// Resize the overlay window to a new logical height while keeping the
/// width constant and re-anchoring the bottom edge above the taskbar.
/// Called from the frontend when the language dropdown opens or closes.
#[tauri::command]
fn set_overlay_height(height: f64, app: AppHandle) {
    let Some(window) = app.get_webview_window("overlay") else {
        return;
    };
    let _ = window.set_size(tauri::LogicalSize::new(OVERLAY_WIDTH, height));
    if let Ok(Some(monitor)) = window.primary_monitor() {
        let scale = monitor.scale_factor();
        let size = monitor.size();
        let logical_w = size.width as f64 / scale;
        let logical_h = size.height as f64 / scale;
        let anchor_bottom = work_area_bottom_logical(scale).unwrap_or(logical_h);
        let x = (logical_w - OVERLAY_WIDTH) / 2.0;
        let y = anchor_bottom - height - OVERLAY_BOTTOM_MARGIN;
        let _ = window.set_position(LogicalPosition::new(x, y));
    }
}


#[tauri::command]
fn get_autostart(app: AppHandle) -> Result<bool, String> {
    app.autolaunch().is_enabled().map_err(|e| format!("{e}"))
}

#[tauri::command]
fn set_autostart(enabled: bool, app: AppHandle) -> Result<(), String> {
    let mgr = app.autolaunch();
    if enabled {
        mgr.enable().map_err(|e| format!("{e}"))
    } else {
        mgr.disable().map_err(|e| format!("{e}"))
    }
}

/// Toggle the system-tray icon at runtime. Persists `cfg.hide_tray` so
/// the choice survives restart. We keep the tray icon allocated even
/// when hidden — toggling `set_visible(true)` later is instant, whereas
/// rebuilding the tray would mean re-wiring menus and event handlers.
#[tauri::command]
fn set_tray_visible(
    visible: bool,
    state: tauri::State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    {
        let mut cfg = state.config.lock();
        cfg.hide_tray = !visible;
        config::save(&cfg).map_err(|e| format!("{e:#}"))?;
    }
    if let Some(tray) = app.tray_by_id("bulbul-tray") {
        tray.set_visible(visible).map_err(|e| format!("{e}"))?;
    }
    // The user expects the pill to disappear the moment they toggle
    // "Hide tray" on. We don't track current dictation state here, so
    // we apply idle's visibility rule — if a dictation is in flight
    // when the user toggles, the next emit_status will re-show the
    // overlay correctly.
    if !visible {
        if let Some(overlay) = app.get_webview_window("overlay") {
            let _ = overlay.hide();
        }
    } else if let Some(overlay) = app.get_webview_window("overlay") {
        // Restore the always-visible behaviour when revealing the tray
        // again — even in idle, the pill should be back on screen.
        let _ = overlay.show();
    }
    Ok(())
}

#[tauri::command]
async fn check_for_updates(app: AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_updater::UpdaterExt;
    let updater = app.updater().map_err(|e| format!("{e}"))?;
    match updater.check().await {
        Ok(Some(update)) => Ok(Some(update.version.to_string())),
        Ok(None) => Ok(None),
        Err(e) => Err(format!("{e}")),
    }
}

/// If the background watcher (see `spawn_update_watcher`) has downloaded
/// a newer version, returns its version string for the UI to render in
/// the "update ready" banner. Returns None when no update is staged.
#[tauri::command]
fn get_staged_update_version(state: tauri::State<'_, AppState>) -> Option<String> {
    state.staged_update.lock().as_ref().map(|s| s.version.clone())
}

/// Install the currently-staged update. Tauri's `install` writes the
/// previously-downloaded bytes to a temp file and spawns the installer
/// (passive mode — quick progress bar, then auto-restart). Bulbul's own
/// process is killed by the installer mid-replace, so we never see the
/// `Ok(())` on the happy path — the function returns only on error.
///
/// Called both from the dashboard banner ("Install & restart") and from
/// the tray Quit handler when an update is sitting in the slot.
/// Mac-only: query whether Bulbul currently has Accessibility permission.
/// Polled by the onboarding wizard's Permissions step to enable Continue
/// once granted. On other platforms returns true unconditionally so the
/// wizard's Mac-specific step is effectively a no-op there.
#[cfg(target_os = "macos")]
#[tauri::command]
fn check_accessibility_status_mac() -> bool {
    // SAFETY: AXIsProcessTrusted is a pure-query function, safe from
    // any thread, no preconditions.
    unsafe { accessibility_sys::AXIsProcessTrusted() }
}

#[cfg(not(target_os = "macos"))]
#[tauri::command]
fn check_accessibility_status_mac() -> bool {
    true
}

/// Restart Bulbul cleanly. Used by the onboarding wizard's
/// Accessibility card on Mac because macOS establishes a process's
/// TCC trust state at launch — if Bulbul started while
/// Accessibility was off and the user enables it in Settings
/// afterwards, AXIsProcessTrusted() can still report false for the
/// rest of the process's lifetime. A relaunch is the reliable way
/// to refresh trust.
#[tauri::command]
fn relaunch_app(app: tauri::AppHandle) {
    app.restart();
}

// Pull in AVFoundation so the AVCaptureDevice class symbol below
// resolves at link time. Empty extern block; the actual call uses
// objc2's class-method message send.
#[cfg(target_os = "macos")]
#[link(name = "AVFoundation", kind = "framework")]
extern "C" {}

/// Mac-only: query the live microphone-permission status. Returns one
/// of "granted" | "denied" | "not_determined" | "restricted" matching
/// AVAuthorizationStatus. Polled by the onboarding Permissions step
/// alongside check_accessibility_status_mac so the wizard can flip the
/// mic card to ✓ the moment the user grants access without forcing
/// them to self-confirm via checkbox.
///
/// Other platforms return "granted" unconditionally — the wizard
/// only shows this step on Mac, so non-Mac builds never see the value
/// but the command needs to exist for the invoke_handler! list.
#[cfg(target_os = "macos")]
#[tauri::command]
fn check_microphone_status_mac() -> String {
    use objc2::msg_send;
    use objc2::runtime::AnyClass;
    use objc2_foundation::NSString;

    // SAFETY: AVCaptureDevice.authorizationStatusForMediaType: is
    // documented as a pure-query class method, callable from any
    // thread. AnyClass::get is also thread-safe. The NSString param
    // outlives the message send (dropped at unsafe-block exit).
    unsafe {
        let cls = match AnyClass::get(c"AVCaptureDevice") {
            Some(c) => c,
            None => {
                tracing::warn!(
                    "AVCaptureDevice class not found; AVFoundation linkage may have failed"
                );
                return "unknown".to_string();
            }
        };
        // AVMediaTypeAudio's NSString value is the FourCC "soun".
        // Hardcoded here to avoid linking just for the constant.
        let media_type = NSString::from_str("soun");
        let status: i64 = msg_send![cls, authorizationStatusForMediaType: &*media_type];
        match status {
            0 => "not_determined",
            1 => "restricted",
            2 => "denied",
            3 => "granted",
            _ => "unknown",
        }
        .to_string()
    }
}

#[cfg(not(target_os = "macos"))]
#[tauri::command]
fn check_microphone_status_mac() -> String {
    "granted".to_string()
}

/// Mac-only: trigger the macOS mic-permission prompt by calling
/// `AVCaptureDevice.requestAccessForMediaType:completionHandler:`.
///
/// This is necessary in addition to the status check because macOS only
/// adds Bulbul to the System Settings → Privacy → Microphone list once
/// the app has actually called requestAccess at least once. Without
/// this, the user clicks "Open Microphone Settings", sees the right
/// pane, but no Bulbul row to toggle — Accessibility doesn't have this
/// quirk because AXIsProcessTrusted's prompt option registers us.
///
/// Fire-and-forget: the completion block is a no-op. The wizard polls
/// `check_microphone_status_mac` every 1.5s and updates the UI when
/// the user responds to the OS prompt (or toggles the slider later).
#[cfg(target_os = "macos")]
#[tauri::command]
fn request_microphone_access_mac() {
    use block2::RcBlock;
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, Bool};
    use objc2_foundation::NSString;

    // SAFETY: AVCaptureDevice.requestAccessForMediaType:completionHandler:
    // is documented as callable from any thread. The completion block is
    // RcBlock-allocated so it lives on the heap until AVFoundation
    // releases its retain after invoking it.
    unsafe {
        let Some(cls) = AnyClass::get(c"AVCaptureDevice") else {
            tracing::warn!(
                "AVCaptureDevice class not found; cannot request mic access"
            );
            return;
        };
        let media_type = NSString::from_str("soun");
        // objc2::runtime::Bool (not plain Rust bool) is required here:
        // block2::RcBlock::new needs every arg type to implement Encode,
        // which Rust's bool does not in the objc2 0.6 / block2 0.6 setup.
        let block = RcBlock::new(|_granted: Bool| {});
        let _: () = msg_send![
            cls,
            requestAccessForMediaType: &*media_type,
            completionHandler: &*block,
        ];
    }
}

#[cfg(not(target_os = "macos"))]
#[tauri::command]
fn request_microphone_access_mac() {}

/// Mac-only: open a specific System Settings privacy pane.
///
/// `pane` is "accessibility", "microphone", or "privacy" (root).
///
/// Uses the classic `com.apple.preference.security` URL handler with
/// `Privacy_X` anchors. macOS keeps this URL as a compat shim even
/// after the Ventura rename to System Settings — it's internally
/// routed to the current Privacy & Security pane and the anchor names
/// are kept stable across versions, including Tahoe. The newer
/// `com.apple.settings.PrivacySecurity.extension` pane id is less
/// reliable: anchor resolution varies by macOS version, and since
/// `open` always exits 0 once it hands the URL off, a silently
/// ignored anchor can't be detected from the caller side.
#[cfg(target_os = "macos")]
#[tauri::command]
fn open_mac_settings_pane(pane: String) -> Result<(), String> {
    use std::process::Command;
    let url = match pane.as_str() {
        "accessibility" => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
        }
        "microphone" => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"
        }
        "privacy" => "x-apple.systempreferences:com.apple.preference.security",
        other => return Err(format!("unknown settings pane: {other}")),
    };
    Command::new("open")
        .arg(url)
        .status()
        .map_err(|e| format!("open spawn failed for {url}: {e}"))
        .and_then(|s| {
            if s.success() {
                Ok(())
            } else {
                Err(format!("open exited {s} for {url}"))
            }
        })
}

#[cfg(not(target_os = "macos"))]
#[tauri::command]
fn open_mac_settings_pane(_pane: String) -> Result<(), String> {
    Ok(())
}

#[tauri::command]
async fn install_staged_update(app: AppHandle) -> Result<(), String> {
    let slot = app.state::<AppState>().staged_update.clone();
    let staged = slot.lock().take();
    let Some(staged) = staged else {
        return Err("no update is staged".into());
    };
    // `install` moves the Update and the bytes. From here, the installer
    // process is in the driver's seat.
    staged
        .update
        .install(staged.bytes)
        .map_err(|e| format!("{e}"))?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,bulbul_lib=debug")),
        )
        .try_init();

    let initial_config = config::load();
    let has_key_on_boot = initial_config.has_api_key();
    let initial_set = HotkeySet {
        dictation: ParsedHotkey::parse(&initial_config.hotkey),
        polish_dictation: ParsedHotkey::parse(&initial_config.polish_hotkey),
        transform_bindings: Vec::new(),
    };
    let hotkey_mutex = Arc::new(Mutex::new(initial_set));
    let (hotkey_tx, hotkey_rx) = hotkey::make_channel();
    let hotkey_rx_for_setup = Mutex::new(Some(hotkey_rx));

    tauri::Builder::default()
        // Single-instance MUST be the first plugin registered. When a second
        // Bulbul launch is attempted, this callback fires inside the already-
        // running process and the new process exits immediately — before it
        // can touch the global hotkey, the modifier-chord watcher, the
        // shared SQLite db, the tray, or the config file. Focusing the
        // existing main window gives the user visible feedback so the
        // double-launch doesn't feel like nothing happened.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            check_accessibility_status_mac,
            check_microphone_status_mac,
            request_microphone_access_mac,
            open_mac_settings_pane,
            relaunch_app,
            get_config,
            save_config,
            validate_api_key,
            complete_onboarding,
            track_event,
            check_for_updates,
            get_staged_update_version,
            install_staged_update,
            get_autostart,
            set_autostart,
            set_tray_visible,
            show_settings_window,
            set_overlay_height,
            get_home_stats,
            get_recent_dictations,
            get_insights_usage,
            get_voice_stats,
            refresh_voice_narrative,
            list_dictionary,
            add_dictionary_entry,
            update_dictionary_entry,
            delete_dictionary_entry,
            correction_suggestions,
            dismiss_correction_suggestion,
            list_corrections,
            list_snippets,
            add_snippet,
            update_snippet,
            delete_snippet,
            list_transforms,
            add_transform,
            update_transform,
            delete_transform,
            run_transform_on_text,
            set_default_transform,
            reset_transforms,
            list_transform_slot_statuses,
            list_notes,
            get_note,
            create_note,
            update_note,
            delete_note,
            open_scratchpad,
        ])
        .setup(move |app| {
            let handle = app.handle().clone();

            // Build tray-icon variants. Mac uses a dedicated monochrome
            // template asset (black silhouette on transparent) so the
            // menu bar can recolor it for dark mode and selected
            // states. Windows + Linux use the bundled colored icon and
            // tint it red to flag "missing API key" state.
            //
            // The "no key" indicator differs by platform:
            //   - Win/Linux: tinted-red variant of the same icon
            //   - Mac: same template icon (state surfaced via tooltip)
            //     since you can't tint a template — the OS overrides
            //     the color anyway. tray.set_icon also can't preserve
            //     the template flag mid-run, so we leave the icon
            //     alone on Mac and let the tooltip carry state.
            #[cfg(target_os = "macos")]
            let active_icon = {
                let img = tauri::image::Image::from_bytes(include_bytes!(
                    "../icons/tray-icon@2x.png"
                ))
                .expect("tray-icon@2x.png failed to decode");
                OwnedIcon::from_image(&img)
            };
            #[cfg(not(target_os = "macos"))]
            let active_icon = {
                let default_icon = handle
                    .default_window_icon()
                    .expect("default window icon must be available")
                    .to_owned();
                OwnedIcon::from_image(&default_icon)
            };
            #[cfg(target_os = "macos")]
            let no_key_icon = active_icon.clone();
            #[cfg(not(target_os = "macos"))]
            let no_key_icon = active_icon.tinted_red();
            let icons = Arc::new(IconVariants {
                active: active_icon,
                no_key: no_key_icon,
            });

            let db_handle = match db::open() {
                Ok(d) => d,
                Err(e) => {
                    tracing::error!("could not open sqlite db: {e:#}");
                    return Err(format!("db open: {e:#}").into());
                }
            };

            let slot_statuses_arc =
                Arc::new(Mutex::new(Vec::<hotkey::TransformSlotStatus>::new()));

            handle.manage(AppState {
                config: Arc::new(Mutex::new(initial_config)),
                hotkeys: hotkey_mutex.clone(),
                hotkey_tx: hotkey_tx.clone(),
                transform_slot_statuses: slot_statuses_arc.clone(),
                icons,
                db: db_handle,
                regex_cache: Arc::new(db::RegexCache::new()),
                staged_update: Arc::new(Mutex::new(None)),
            });

            // Warm the dictionary/snippet regex caches in the background so the
            // first dictation of the session isn't slower than every one after.
            {
                let state = handle.state::<AppState>();
                let regex_cache = state.regex_cache.clone();
                let db = state.db.clone();
                std::thread::spawn(move || regex_cache.warm(&db));
            }

            // Initial registration happens here; refresh_transform_bindings
            // below also re-runs install_global_shortcuts so the slot
            // hotkeys get wired up the first time. Both runs are idempotent
            // because install_global_shortcuts unregisters everything
            // first.
            let _ = hotkey::install_global_shortcuts(
                &handle,
                hotkey_mutex.clone(),
                hotkey_tx.clone(),
            );

            setup_tray(&handle, has_key_on_boot)?;
            setup_overlay_window(&handle)?;
            setup_scratchpad_window(&handle)?;

            if let Some(window) = handle.get_webview_window("main") {
                let cfg = handle.state::<AppState>().config.clone();
                let want_show = {
                    let c = cfg.lock();
                    !c.has_api_key() || c.open_dashboard_on_launch
                };
                if want_show {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
                let win_handle = window.clone();
                window.on_window_event(move |event| {
                    if let WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        let _ = win_handle.hide();
                    }
                });
            }

            // Initial transform slot hotkey bindings.
            refresh_transform_bindings(&handle, &handle.state::<AppState>());

            let rx = hotkey_rx_for_setup
                .lock()
                .take()
                .expect("hotkey rx already consumed");
            spawn_orchestrator(handle.clone(), rx);
            spawn_hover_watcher(handle.clone());

            // Mode-B auto-update: silently poll GitHub Releases on a
            // 6-hour cadence (10s grace after boot), download new
            // installers into AppState.staged_update, fire `update-staged`
            // event. The UI banner and the tray Quit handler do the rest.
            spawn_update_watcher(handle.clone());

            // Telemetry boot. The opt-in toggle is per-call, but we always
            // start the periodic flush so any track() calls that happen
            // while opted in get drained on a steady cadence. If the user
            // is opted out, the buffer never fills (no one calls track),
            // so the flush is a no-op.
            telemetry::spawn_periodic_flush();
            {
                let state = handle.state::<AppState>();
                let cfg = state.config.lock();
                if cfg.telemetry_enabled {
                    telemetry::track(
                        "app_started",
                        serde_json::json!({
                            "has_api_key": cfg.has_api_key(),
                            "mode": cfg.mode.as_str(),
                            "language": cfg.language,
                            "theme": cfg.theme,
                            "onboarding_completed": cfg.onboarding_completed,
                            "style_enabled": cfg.style_enabled,
                            "personalize_cleanup": cfg.personalize_cleanup,
                            "open_dashboard_on_launch": cfg.open_dashboard_on_launch,
                        }),
                    );
                }
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Bulbul");
}

fn setup_tray(app: &AppHandle, has_key: bool) -> tauri::Result<()> {
    let settings = MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?;
    let check_update = MenuItem::with_id(app, "check_update", "Check for updates", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Bulbul", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&settings, &check_update, &quit])?;

    let state = app.state::<AppState>();
    let initial_icon = if has_key {
        state.icons.active.to_image()
    } else {
        state.icons.no_key.to_image()
    };
    let tooltip = if has_key {
        "Bulbul — hold your hotkey to dictate"
    } else {
        "Bulbul — set your Groq API key in Settings"
    };

    let initial_visible = !app.state::<AppState>().config.lock().hide_tray;
    // `mut` is unused on non-Mac (the cfg-gated reassignment below
    // compiles away). Suppress the lint rather than duplicate the
    // whole builder chain.
    #[allow(unused_mut)]
    let mut tray_builder = TrayIconBuilder::with_id("bulbul-tray")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .tooltip(tooltip)
        .icon(initial_icon);
    #[cfg(target_os = "macos")]
    {
        // Mac: render the icon as an NSImage template so the menu bar
        // owns the color (auto-inverts for dark mode, highlights when
        // selected, etc). Requires the icon to be a pure-black
        // silhouette on transparent, which tray-icon@2x.png is.
        tray_builder = tray_builder.icon_as_template(true);
    }
    let tray = tray_builder
        .on_menu_event(|app, event| match event.id.as_ref() {
            "settings" => show_settings(app),
            "check_update" => {
                let app_clone = app.clone();
                tauri::async_runtime::spawn(async move {
                    use tauri_plugin_updater::UpdaterExt;
                    let result = match app_clone.updater() {
                        Ok(u) => u.check().await,
                        Err(e) => {
                            notify(&app_clone, "Update check failed", &format!("{e}"));
                            return;
                        }
                    };
                    match result {
                        Ok(Some(update)) => notify(
                            &app_clone,
                            "Update available",
                            &format!("Bulbul v{} is available — open Settings to install.", update.version),
                        ),
                        Ok(None) => notify(&app_clone, "Bulbul is up to date", ""),
                        Err(e) => notify(&app_clone, "Update check failed", &format!("{e}")),
                    }
                });
            }
            "quit" => {
                // Mode-B promise: "install on next restart". If an update
                // is already downloaded, the installer takes over from
                // here — passive mode runs ~3s of UI then relaunches.
                install_staged_if_present(app);
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: tauri::tray::MouseButton::Left,
                button_state: tauri::tray::MouseButtonState::Up,
                ..
            } = event
            {
                show_settings(tray.app_handle());
            }
        })
        .build(app)?;
    // Tauri 2's TrayIconBuilder has no .visible() — apply the
    // hide_tray preference after build. Best-effort: if the platform
    // refuses to hide, we log and keep going (tray simply stays
    // visible until the user retries).
    if let Err(e) = tray.set_visible(initial_visible) {
        tracing::warn!("could not apply initial tray visibility: {e}");
    }
    Ok(())
}

fn setup_overlay_window(app: &AppHandle) -> tauri::Result<()> {
    let overlay = WebviewWindowBuilder::new(
        app,
        "overlay",
        WebviewUrl::App("index.html#overlay".into()),
    )
    .title("Bulbul Overlay")
    .inner_size(OVERLAY_WIDTH, OVERLAY_HEIGHT)
    .decorations(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .resizable(false)
    .transparent(true)
    .shadow(false)
    .visible(true)
    .focused(false)
    .build()?;
    let _ = overlay.set_ignore_cursor_events(true);
    position_overlay_bottom_center(app);
    Ok(())
}

fn show_settings(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn spawn_orchestrator(handle: AppHandle, rx: std::sync::mpsc::Receiver<HotkeyEvent>) {
    // The receiver is wired up at boot in `setup`; the plugin's press
    // handlers + release poller (see hotkey.rs) send events through it.

    thread::spawn(move || {
        // meta.mode carries the cleanup mode chosen at press time:
        //   - DictationPressed       → cfg.mode (whatever the user set)
        //   - PolishDictationPressed → CleanupMode::Polished, forced
        // Release handlers ignore which key fired and just process the
        // active recording with whatever meta.mode says.
        let mut active: Option<(Recorder, PendingDictation)> = None;
        for evt in rx {
            match evt {
                HotkeyEvent::DictationPressed | HotkeyEvent::PolishDictationPressed => {
                    // Cross-key race guard: only one recording in flight at
                    // a time. (Each hotkey's plugin handler has its own
                    // auto-repeat guard; this catches near-simultaneous
                    // presses of both hotkeys.)
                    if active.is_some() {
                        continue;
                    }
                    let cfg_arc = handle.state::<AppState>().config.clone();
                    let cfg = cfg_arc.lock().clone();
                    if !cfg.has_api_key() {
                        emit_status(
                            &handle,
                            "error",
                            Some("Set your Groq API key in Settings first.".into()),
                        );
                        notify(&handle, "Bulbul", "Set your Groq API key in Settings first.");
                        show_settings(&handle);
                        continue;
                    }
                    let cleanup_mode = if matches!(evt, HotkeyEvent::PolishDictationPressed) {
                        CleanupMode::Polished
                    } else {
                        cfg.mode.clone()
                    };
                    match Recorder::start() {
                        Ok(rec) => {
                            tracing::info!("recording started (mode={cleanup_mode:?})");
                            emit_status(&handle, "listening", None);
                            let meta = PendingDictation {
                                started_at: Instant::now(),
                                foreground_app: window_info::foreground_app(),
                                language: cfg.language.clone(),
                                mode: cleanup_mode,
                            };
                            active = Some((rec, meta));
                        }
                        Err(e) => {
                            tracing::error!("recorder start failed: {e:#}");
                            emit_status(&handle, "error", Some(format!("{e:#}")));
                            notify(&handle, "Bulbul mic error", &format!("{e:#}"));
                        }
                    }
                }
                HotkeyEvent::DictationReleased | HotkeyEvent::PolishDictationReleased => {
                    let Some((rec, meta)) = active.take() else {
                        continue;
                    };
                    let captured = rec.captured_seconds();
                    let cfg_arc = handle.state::<AppState>().config.clone();
                    let mut cfg = cfg_arc.lock().clone();
                    // Honor the cleanup mode chosen at press time — the
                    // polish hotkey forces Polished even if the user's
                    // global mode is Raw or Clean.
                    cfg.mode = meta.mode.clone();
                    if captured < cfg.min_recording_seconds {
                        tracing::info!(
                            "discarding {:.2}s clip (min {:.2}s)",
                            captured,
                            cfg.min_recording_seconds
                        );
                        emit_status(&handle, "idle", Some("Too short, ignored.".into()));
                        continue;
                    }
                    let result = match rec.finish() {
                        Ok(r) => r,
                        Err(e) => {
                            tracing::error!("encoding WAV failed: {e:#}");
                            emit_status(&handle, "error", Some(format!("{e:#}")));
                            notify(&handle, "Bulbul audio error", &format!("{e:#}"));
                            continue;
                        }
                    };
                    // Energy gate: drop near-silent clips before they reach
                    // Whisper — otherwise it hallucinates "thank you" / "you" /
                    // "thanks for watching" / Hindi-equivalent phrases that
                    // can slip past the post-STT denylist. We require BOTH a
                    // tolerable peak AND a tolerable RMS: peak alone misses
                    // "quiet room with a brief click" (a single mouse click
                    // can push peak above -55 dBFS while average energy stays
                    // at room-tone), and RMS alone misses sustained low drones.
                    // Real speech has both; ambient noise has only one.
                    //
                    // Thresholds tuned conservatively so quiet-but-real speech
                    // still passes: normal speech is ~-25 dBFS RMS, soft
                    // speech ~-35 dBFS, true ambient room noise ~-50 dBFS.
                    const SILENCE_PEAK_DBFS: f32 = -50.0;
                    const SILENCE_RMS_DBFS: f32 = -42.0;
                    if result.peak_dbfs < SILENCE_PEAK_DBFS
                        || result.rms_dbfs < SILENCE_RMS_DBFS
                    {
                        tracing::info!(
                            "discarding silent clip (peak={:.1} dBFS, rms={:.1} dBFS, {:.2}s)",
                            result.peak_dbfs,
                            result.rms_dbfs,
                            result.seconds
                        );
                        emit_status(&handle, "idle", Some("Silence — nothing to transcribe.".into()));
                        continue;
                    }
                    let duration_ms = meta.started_at.elapsed().as_millis() as u64;
                    emit_status(&handle, "processing", None);
                    let handle_for_task = handle.clone();
                    let wav = result.wav;
                    tauri::async_runtime::spawn(async move {
                        process_pipeline(handle_for_task, cfg, wav, meta, duration_ms).await;
                    });
                }
                HotkeyEvent::TransformTriggered(transform_id) => {
                    tracing::info!("TransformTriggered received: id={}", transform_id);
                    let state = handle.state::<AppState>();
                    let cfg = state.config.lock().clone();
                    if !cfg.has_api_key() {
                        emit_status(
                            &handle,
                            "error",
                            Some("Set your Groq API key in Settings first.".into()),
                        );
                        notify(&handle, "Bulbul", "Set your Groq API key in Settings first.");
                        show_settings(&handle);
                        continue;
                    }
                    let transform = match db::get_transform(&state.db, transform_id) {
                        Ok(t) => Some(t),
                        Err(e) => {
                            tracing::warn!("transform id {transform_id} missing: {e:#}");
                            None
                        }
                    };
                    let handle_for_task = handle.clone();
                    tauri::async_runtime::spawn(async move {
                        transform_pipeline(handle_for_task, cfg, transform).await;
                    });
                }
            }
        }
    });
}

async fn transform_pipeline(app: AppHandle, cfg: Config, transform: Option<db::Transform>) {
    let t_pipeline_start = Instant::now();
    let transform_name = transform
        .as_ref()
        .map(|t| t.name.clone())
        .unwrap_or_else(|| "<fallback>".into());
    tracing::info!("transform_pipeline start: transform={transform_name}");
    // Reuse a single Clipboard handle across save / clear / read / paste /
    // restore. Repeatedly opening arboard on Windows triggers OLE init/teardown
    // cycles that can corrupt the heap when paired with rdev's keyboard hook.
    let mut clipboard = match arboard::Clipboard::new() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("clipboard open failed: {e:#}");
            emit_status(&app, "error", Some(format!("Clipboard: {e:#}")));
            return;
        }
    };

    let original = clipboard.get_text().ok();
    let _ = clipboard.set_text(String::new());

    let t_capture_start = Instant::now();
    if let Err(e) = inject::send_ctrl_c() {
        emit_status(&app, "error", Some(format!("Ctrl+C failed: {e:#}")));
        notify(&app, "Bulbul polish failed", &format!("{e:#}"));
        restore_clipboard_with(&mut clipboard, original);
        return;
    }

    // Give the foreground app a moment to populate the clipboard.
    tokio::time::sleep(Duration::from_millis(220)).await;

    let selected = clipboard.get_text().unwrap_or_default();
    let t_capture_ms = t_capture_start.elapsed().as_millis() as u64;

    if selected.trim().is_empty() {
        tracing::warn!("transform_pipeline: clipboard empty after Ctrl+C (no selection captured)");
        emit_status(
            &app,
            "error",
            Some("No text selected — highlight something first.".into()),
        );
        notify(&app, "Bulbul polish", "No text selected — highlight some text and try again.");
        restore_clipboard_with(&mut clipboard, original);
        return;
    }
    let input_chars = selected.chars().count();
    tracing::info!(
        "transform[{}] input ({} chars): {:?}",
        transform_name,
        selected.len(),
        selected.chars().take(200).collect::<String>()
    );

    let db = app.state::<AppState>().db.clone();
    const FALLBACK_PROMPT: &str = "Polish the user's text: fix grammar, improve flow, preserve meaning. Return only the rewritten text.";
    let prompt = transform.as_ref().map(|t| t.system_prompt.as_str()).unwrap_or(FALLBACK_PROMPT);

    emit_status(&app, "processing", None);
    let t_llm_start = Instant::now();
    let rl_app = app.clone();
    let on_rate_limit = move |secs: u64| {
        emit_status(&rl_app, "rate_limited", Some(format!("Rate limited · {secs}s")));
    };
    let notify_rl: &groq::RetryNotify = &on_rate_limit;
    let polished = match groq::execute_transform(&cfg, prompt, &selected, Some(notify_rl)).await {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("transform failed: {e:#}");
            emit_status(&app, "error", Some(format!("{e:#}")));
            notify(&app, "Bulbul transform failed", &format!("{e:#}"));
            restore_clipboard_with(&mut clipboard, original);
            return;
        }
    };
    let t_llm_ms = t_llm_start.elapsed().as_millis() as u64;
    tracing::info!(
        "transform output ({} chars): {:?}",
        polished.len(),
        polished.chars().take(200).collect::<String>()
    );
    if let Some(t) = &transform {
        let _ = db::bump_transform_hits(&db, t.id);
    }

    if polished.trim().is_empty() {
        emit_status(&app, "error", Some("Transform returned empty text.".into()));
        restore_clipboard_with(&mut clipboard, original);
        return;
    }

    let regex_cache = app.state::<AppState>().regex_cache.clone();
    let (final_text, dict_hits) = regex_cache.apply_dictionary(&db, &polished);
    if !dict_hits.is_empty() {
        let _ = db::bump_dictionary_hits(&db, &dict_hits);
    }

    emit_status(&app, "injecting", None);
    let t_inject_start = Instant::now();
    if let Err(e) = clipboard.set_text(final_text.clone()) {
        tracing::error!("clipboard write failed: {e:#}");
        emit_status(&app, "error", Some(format!("Clipboard: {e:#}")));
        restore_clipboard_with(&mut clipboard, original);
        return;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    if let Err(e) = inject::send_ctrl_v() {
        tracing::error!("Ctrl+V failed: {e:#}");
        emit_status(&app, "error", Some(format!("Paste: {e:#}")));
        notify(&app, "Bulbul transform failed", &format!("{e:#}"));
        restore_clipboard_with(&mut clipboard, original);
        return;
    }
    let t_inject_ms = t_inject_start.elapsed().as_millis() as u64;

    emit_status(&app, "done", Some(final_text.clone()));

    let t_total_ms = t_pipeline_start.elapsed().as_millis() as u64;
    let out_chars = final_text.chars().count();
    tracing::info!(
        "perf-transform[{}]: total={}ms capture={}ms llm={}ms inject={}ms | in={}c out={}c",
        transform_name,
        t_total_ms,
        t_capture_ms,
        t_llm_ms,
        t_inject_ms,
        input_chars,
        out_chars
    );

    // Drop the pipeline's clipboard handle so the background restore can
    // open its own. Then async-defer the 250ms wait + restore. Safety
    // guard: only restore if the clipboard still holds our paste — that
    // way nothing the user copies in between gets overwritten.
    drop(clipboard);
    if let Some(orig) = original {
        let our_paste = final_text;
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(250));
            let Ok(mut cb) = arboard::Clipboard::new() else {
                return;
            };
            match cb.get_text() {
                Ok(current) if current == our_paste => {
                    let _ = cb.set_text(orig);
                }
                _ => {}
            }
        });
    }
}

fn restore_clipboard_with(clipboard: &mut arboard::Clipboard, original: Option<String>) {
    if let Some(orig) = original {
        let _ = clipboard.set_text(orig);
    }
}


/// Format (raw, cleaned) pairs into a few-shot block injected into the
/// cleanup system prompt. The model sees examples of how this user's
/// dictations have historically been cleaned in the same app + mode, and
/// is asked to match the tone/vocabulary. Each text capped at 280 chars so
/// a long historical paste can't blow the prompt budget.
fn format_style_memory(pairs: &[(String, String)]) -> Option<String> {
    if pairs.is_empty() {
        return None;
    }
    let lines: Vec<String> = pairs
        .iter()
        // Oldest of the recent set first so the most-recent example is
        // closest to the actual instruction — recency bias works for us.
        .rev()
        .map(|(raw, clean)| {
            let r: String = raw.chars().take(280).collect();
            let c: String = clean.chars().take(280).collect();
            format!("Raw: {}\nCleaned: {}", r.trim(), c.trim())
        })
        .collect();
    Some(format!(
        "Recent examples of how this user's dictations have been cleaned \
         in this context. Match their vocabulary, punctuation habits, and \
         formality. Do NOT copy content from these examples into the new output \
         — they are style reference only:\n\n{}",
        lines.join("\n\n")
    ))
}

/// Format the user's past hand-corrections (V3.1 correction memory) into a
/// prompt block. Currently unused — the few-shot apply path was disabled
/// after it caused the small cleanup model to echo example text. Retained for
/// the upcoming safe apply redesign.
#[allow(dead_code)]
fn format_corrections(pairs: &[(String, String)]) -> Option<String> {
    if pairs.is_empty() {
        return None;
    }
    let lines: Vec<String> = pairs
        .iter()
        .map(|(injected, corrected)| {
            let i: String = injected.chars().take(280).collect();
            let c: String = corrected.chars().take(280).collect();
            format!("Before: {}\nAfter: {}", i.trim(), c.trim())
        })
        .collect();
    Some(format!(
        "This user has previously hand-corrected your output. When the same \
         words or patterns come up, apply the same change so they don't have to \
         fix it again. These are corrections to learn from, not text to copy:\n\n{}",
        lines.join("\n\n")
    ))
}

async fn process_pipeline(
    app: AppHandle,
    cfg: Config,
    wav: Vec<u8>,
    meta: PendingDictation,
    duration_ms: u64,
) {
    let t_pipeline_start = Instant::now();
    let audio_bytes = wav.len();
    // Hand the dictionary's canonical spellings to Whisper as a prompt hint —
    // biases the STT toward the user's preferred forms (e.g. "Groq", "iOS").
    let db = app.state::<AppState>().db.clone();
    let vocabulary: Vec<String> = db::list_dictionary(&db)
        .unwrap_or_default()
        .into_iter()
        .map(|e| e.to_word)
        .collect();

    // Shared across STT + cleanup: tell the overlay we're backing off on a
    // Groq rate limit instead of letting the dictation appear to hang.
    let rl_app = app.clone();
    let on_rate_limit = move |secs: u64| {
        emit_status(&rl_app, "rate_limited", Some(format!("Rate limited · {secs}s")));
    };
    let notify_rl: &groq::RetryNotify = &on_rate_limit;

    let t_stt_start = Instant::now();
    let transcript = match groq::transcribe(&cfg, wav, &vocabulary, Some(notify_rl)).await {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("STT failed: {e:#}");
            emit_status(&app, "error", Some(format!("STT: {e:#}")));
            notify(&app, "Bulbul transcription failed", &format!("{e:#}"));
            track_dictation_failed(&app, "stt", &meta.mode);
            return;
        }
    };
    let t_stt_ms = t_stt_start.elapsed().as_millis() as u64;
    tracing::debug!("raw transcript: {transcript}");
    if transcript.trim().is_empty() || groq::is_likely_hallucination(&transcript) {
        tracing::info!("dropping likely-hallucinated transcript: {transcript:?}");
        emit_status(&app, "idle", Some("No speech detected.".into()));
        return;
    }

    // Build the extra-context block appended to the cleanup system prompt.
    // Two independent contributions:
    //   1. Style preset (formal/casual/very-casual) inferred from the
    //      foreground app, when style_enabled.
    //   2. Few-shot personalization examples from the user's own past
    //      dictations in this same app + mode, when personalize_cleanup.
    // Concatenated with blank lines so the model reads them as separate
    // instructions rather than one run-on block.
    let mut style_parts: Vec<String> = Vec::new();
    if cfg.style_enabled {
        let category = cfg.category_for_app(meta.foreground_app.as_deref());
        let key = cfg.style_for_category(category);
        if let Some(m) = config::style_modifier(key) {
            style_parts.push(m.to_string());
        }
    }
    if cfg.personalize_cleanup && !matches!(cfg.mode, CleanupMode::Raw) {
        let pairs = db::style_memory(
            &db,
            meta.foreground_app.as_deref(),
            cfg.mode.as_str(),
            3,
        )
        .unwrap_or_default();
        if let Some(block) = format_style_memory(&pairs) {
            tracing::info!(
                "personalization: {} few-shot example(s) for app={:?} mode={}",
                pairs.len(),
                meta.foreground_app.as_deref().unwrap_or("(none)"),
                cfg.mode.as_str()
            );
            style_parts.push(block);
        }
    }
    // Correction memory (V3.1): the apply path is intentionally disabled.
    // Injecting past corrections as few-shot Before/After pairs caused
    // `llama-3.1-8b-instant` to emit the example text verbatim instead of
    // cleaning the real transcript (observed: a "Cloud correctly" dictation
    // came out as a stored correction's text). Capture/storage still runs so
    // the data accrues; a safe apply mechanism is pending redesign.
    // See `db::relevant_corrections` / `format_corrections` (kept for reuse).
    let style_extra: Option<String> = if style_parts.is_empty() {
        None
    } else {
        Some(style_parts.join("\n\n"))
    };

    // Venue hint: tell the cleanup model which app the text is being pasted
    // into so it can adapt formatting conventions (no markdown in shells,
    // paragraphs in email, terse in chat) without us authoring per-app rules.
    // Gated on the same toggle as Style — flipping that off disables all
    // per-app behavior in one switch.
    let app_context: Option<String> = if cfg.style_enabled {
        meta.foreground_app.as_deref().map(|exe| {
            let name = config::friendly_app_name(exe);
            format!(
                "Venue: The user's cleaned text will be pasted into {name}. \
                 Adapt formatting (markdown, code blocks, quotes, line breaks, \
                 punctuation, greeting/sign-off) to that app's conventions. \
                 Do not invent content the speaker did not say."
            )
        })
    } else {
        None
    };
    if let Some(ctx) = &app_context {
        tracing::info!("app context: {}", ctx);
    }

    let t_cleanup_start = Instant::now();
    let cleaned = match groq::cleanup(
        &cfg,
        &transcript,
        style_extra.as_deref(),
        app_context.as_deref(),
        Some(notify_rl),
    ).await {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!("cleanup failed, falling back to raw: {e:#}");
            transcript.clone()
        }
    };
    let t_cleanup_ms = t_cleanup_start.elapsed().as_millis() as u64;
    tracing::debug!("cleaned: {cleaned}");

    let t_local_start = Instant::now();
    let regex_cache = app.state::<AppState>().regex_cache.clone();
    let (with_dict, dict_hits) = regex_cache.apply_dictionary(&db, &cleaned);
    if !dict_hits.is_empty() {
        tracing::debug!(
            "dictionary applied {} fix(es)",
            dict_hits.iter().map(|(_, c)| c).sum::<i64>()
        );
    }
    let (final_text, snip_hits) = regex_cache.apply_snippets(&db, &with_dict);
    if !snip_hits.is_empty() {
        tracing::debug!(
            "snippets expanded {} time(s)",
            snip_hits.iter().map(|(_, c)| c).sum::<i64>()
        );
    }
    // DB writes deferred: the hit-counter UPDATEs + activity-log INSERT
    // happen in one transaction below, after injection, so the user doesn't
    // wait on three sequential fsyncs.
    let t_local_ms = t_local_start.elapsed().as_millis() as u64;

    emit_status(&app, "injecting", None);
    let t_inject_start = Instant::now();
    if let Err(e) = inject::inject_text(&final_text) {
        tracing::error!("inject failed: {e:#}");
        emit_status(&app, "error", Some(format!("Inject: {e:#}")));
        notify(&app, "Bulbul inject failed", &format!("{e:#}"));
        track_dictation_failed(&app, "inject", &meta.mode);
        return;
    }
    let t_inject_ms = t_inject_start.elapsed().as_millis() as u64;

    // Correction memory (V3.1): watch the field we just pasted into for edits
    // the user makes, on a background thread, and store any clean correction.
    // Spawned right after injection so the snapshot sees our fresh paste.
    if cfg.learn_corrections {
        correction::watch_for_correction(
            final_text.clone(),
            meta.foreground_app.clone(),
            db.clone(),
        );
    }

    let t_total_ms = t_pipeline_start.elapsed().as_millis() as u64;
    let word_count = final_text.split_whitespace().count();
    // End-to-end latency the user actually perceives: from hotkey release
    // (= recording stopped, process_pipeline entered) to text on screen.
    // Logged in a single line so it's easy to grep / paste into a sheet
    // when comparing latency across hardware.
    tracing::info!(
        "perf: total={}ms stt={}ms cleanup={}ms local={}ms inject={}ms | audio_dur={}ms audio_bytes={} words={}",
        t_total_ms,
        t_stt_ms,
        t_cleanup_ms,
        t_local_ms,
        t_inject_ms,
        duration_ms,
        audio_bytes,
        word_count
    );

    // Snapshot the meta fields we need for telemetry BEFORE the
    // LogEntry below consumes them. Cheap clones — short strings.
    let telemetry_payload = if cfg.telemetry_enabled {
        let venue_category = cfg.category_for_app(meta.foreground_app.as_deref());
        Some(serde_json::json!({
            "mode": meta.mode.as_str(),
            "language": meta.language.clone(),
            "duration_bucket": telemetry::duration_bucket(duration_ms),
            "word_count_bucket": telemetry::word_count_bucket(word_count),
            "had_dict_hits": !dict_hits.is_empty(),
            "had_snippet_hits": !snip_hits.is_empty(),
            "venue_category": venue_category,
            "stt_ms_bucket": telemetry::duration_bucket(t_stt_ms),
            "cleanup_ms_bucket": telemetry::duration_bucket(t_cleanup_ms),
        }))
    } else {
        None
    };

    // Atomic activity-log + hit-counter write. Single transaction so SQLite
    // does one fsync instead of three. Best-effort — failures never block
    // injection or surface to the user.
    if let Err(e) = db::log_dictation_with_hits(
        &db,
        db::LogEntry {
            raw_text: transcript,
            cleaned_text: final_text.clone(),
            mode: meta.mode,
            language: meta.language,
            foreground_app: meta.foreground_app,
            duration_ms,
        },
        &dict_hits,
        &snip_hits,
    ) {
        tracing::warn!("failed to log dictation: {e:#}");
    }

    // Keep the voice profile current without the user having to click Refresh.
    maybe_auto_refresh_voice(&app, &cfg, &db);

    if let Some(props) = telemetry_payload {
        telemetry::track("dictation_completed", props);
    }

    emit_status(&app, "done", Some(final_text));
}

/// Fire a telemetry event for a failed dictation. Caller still emits the
/// user-visible error; this only adds an anonymous datapoint.
fn track_dictation_failed(app: &AppHandle, category: &str, mode: &CleanupMode) {
    // Bind the State explicitly so the MutexGuard's borrow outlives the
    // surrounding statement.
    let state = app.state::<AppState>();
    let enabled = state.config.lock().telemetry_enabled;
    if !enabled {
        return;
    }
    telemetry::track(
        "dictation_failed",
        serde_json::json!({
            "error_category": category,
            "mode": mode.as_str(),
        }),
    );
}
