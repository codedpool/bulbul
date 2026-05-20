mod audio;
mod config;
mod db;
mod groq;
mod hotkey;
mod inject;
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
    icons: Arc<IconVariants>,
    db: db::Db,
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
    let _ = app.emit(
        "bulbul-status",
        StatusPayload {
            state,
            message: message.clone(),
        },
    );
    // After a terminal state, fall back to idle so the overlay shrinks.
    if matches!(state, "done" | "error") {
        let app_clone = app.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_millis(1100)).await;
            let _ = app_clone.emit(
                "bulbul-status",
                StatusPayload {
                    state: "idle",
                    message: None,
                },
            );
        });
    }
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
    let state = app.state::<AppState>();
    let icon = if has_key {
        state.icons.active.to_image()
    } else {
        state.icons.no_key.to_image()
    };
    let _ = tray.set_icon(Some(icon));
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
    let (prev_has_key, prev_hotkey, prev_polish) = {
        let cfg = state.config.lock();
        (cfg.has_api_key(), cfg.hotkey.clone(), cfg.polish_hotkey.clone())
    };
    config::save(&new_cfg).map_err(|e| format!("{e:#}"))?;
    let next_has_key = new_cfg.has_api_key();
    let next_hotkey = new_cfg.hotkey.clone();
    let next_polish = new_cfg.polish_hotkey.clone();
    *state.config.lock() = new_cfg;

    if prev_has_key != next_has_key {
        update_tray_icon(&app, next_has_key);
    }
    if prev_hotkey != next_hotkey || prev_polish != next_polish {
        let mut set = state.hotkeys.lock();
        set.dictation = ParsedHotkey::parse(&next_hotkey);
        set.polish = ParsedHotkey::parse(&next_polish);
    }
    Ok(())
}

#[tauri::command]
async fn validate_api_key(api_key: String) -> Result<(), String> {
    groq::validate_key(api_key.trim())
        .await
        .map_err(|e| format!("{e:#}"))
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
    state: tauri::State<'_, AppState>,
) -> Result<Vec<db::DictationRow>, String> {
    db::recent_dictations(&state.db, limit).map_err(|e| format!("{e:#}"))
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

#[tauri::command]
async fn refresh_voice_narrative(app: AppHandle) -> Result<db::VoiceStats, String> {
    let state = app.state::<AppState>();
    let cfg = state.config.lock().clone();
    let db = state.db.clone();
    if !cfg.has_api_key() {
        return Err("Set your Groq API key in Settings first.".into());
    }

    // Gather a stats summary the model can reason about, plus recent samples.
    let stats = db::voice_stats(&db, true).map_err(|e| format!("{e:#}"))?;
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

    let samples = db::voice_profile_context(&db).map_err(|e| format!("{e:#}"))?;

    let (voice_narrative, peak_narrative) =
        groq::generate_voice_profile(&cfg, &stats_summary, &samples)
            .await
            .map_err(|e| format!("{e:#}"))?;

    db::save_voice_narrative(&db, &voice_narrative, &peak_narrative)
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
    db::add_dictionary_entry(&state.db, &from_word, &to_word, case_sensitive)
        .map_err(|e| format!("{e:#}"))
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
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
fn delete_dictionary_entry(
    id: i64,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    db::delete_dictionary_entry(&state.db, id).map_err(|e| format!("{e:#}"))
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
fn polish_now(app: AppHandle) -> Result<(), String> {
    let cfg_arc = app.state::<AppState>().config.clone();
    let cfg = cfg_arc.lock().clone();
    if !cfg.has_api_key() {
        return Err("Set your Groq API key in Settings first.".into());
    }
    let app_clone = app.clone();
    tauri::async_runtime::spawn(async move {
        polish_pipeline(app_clone, cfg).await;
    });
    Ok(())
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
        polish: ParsedHotkey::parse(&initial_config.polish_hotkey),
    };
    let hotkey_mutex = Arc::new(Mutex::new(initial_set));

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .invoke_handler(tauri::generate_handler![
            get_config,
            save_config,
            validate_api_key,
            check_for_updates,
            get_autostart,
            set_autostart,
            polish_now,
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
        ])
        .setup(move |app| {
            let handle = app.handle().clone();

            // Build icon variants from the bundled default window icon.
            let default_icon = handle
                .default_window_icon()
                .expect("default window icon must be available")
                .to_owned();
            let active_icon = OwnedIcon::from_image(&default_icon);
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

            handle.manage(AppState {
                config: Arc::new(Mutex::new(initial_config)),
                hotkeys: hotkey_mutex.clone(),
                icons,
                db: db_handle,
            });

            setup_tray(&handle, has_key_on_boot)?;
            setup_overlay_window(&handle)?;

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

            spawn_orchestrator(handle.clone(), hotkey_mutex.clone());
            spawn_hover_watcher(handle.clone());
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

    let _tray = TrayIconBuilder::with_id("bulbul-tray")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .tooltip(tooltip)
        .icon(initial_icon)
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
            "quit" => app.exit(0),
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

fn spawn_orchestrator(handle: AppHandle, hotkeys: Arc<Mutex<HotkeySet>>) {
    let initial = hotkeys.lock().clone();
    let (live_hotkeys, rx) = hotkey::spawn_listener(initial);

    // Keep listener's hotkeys in sync with the AppState hotkeys mutex.
    {
        let live_hotkeys = live_hotkeys.clone();
        let hotkeys = hotkeys.clone();
        thread::spawn(move || loop {
            thread::sleep(Duration::from_millis(500));
            let current = hotkeys.lock().clone();
            let mut listener = live_hotkeys.lock();
            if listener.dictation != current.dictation
                || listener.polish != current.polish
            {
                *listener = current;
            }
        });
    }

    thread::spawn(move || {
        let mut active: Option<(Recorder, PendingDictation)> = None;
        for evt in rx {
            match evt {
                HotkeyEvent::DictationPressed => {
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
                    match Recorder::start() {
                        Ok(rec) => {
                            tracing::info!("recording started");
                            emit_status(&handle, "listening", None);
                            let meta = PendingDictation {
                                started_at: Instant::now(),
                                foreground_app: window_info::foreground_app(),
                                language: cfg.language.clone(),
                                mode: cfg.mode.clone(),
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
                HotkeyEvent::DictationReleased => {
                    let Some((rec, meta)) = active.take() else {
                        continue;
                    };
                    let captured = rec.captured_seconds();
                    let cfg_arc = handle.state::<AppState>().config.clone();
                    let cfg = cfg_arc.lock().clone();
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
                    // Energy gate: if the recording is essentially silence,
                    // skip the API call entirely. Whisper hallucinates
                    // "thank you" / "you" on silent input.
                    const SILENCE_PEAK_DBFS: f32 = -40.0;
                    if result.peak_dbfs < SILENCE_PEAK_DBFS {
                        tracing::info!(
                            "discarding silent clip ({:.1} dBFS peak, {:.2}s)",
                            result.peak_dbfs,
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
                HotkeyEvent::PolishTriggered => {
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
                    let handle_for_task = handle.clone();
                    tauri::async_runtime::spawn(async move {
                        polish_pipeline(handle_for_task, cfg).await;
                    });
                }
            }
        }
    });
}

async fn polish_pipeline(app: AppHandle, cfg: Config) {
    // Step 1: grab the current selection by simulating Ctrl+C and reading the
    // clipboard. Save and restore the user's existing clipboard around it.
    let original = {
        match arboard::Clipboard::new() {
            Ok(mut c) => c.get_text().ok(),
            Err(_) => None,
        }
    };

    // Clear the clipboard so we can detect whether a real selection exists.
    if let Ok(mut c) = arboard::Clipboard::new() {
        let _ = c.set_text(String::new());
    }

    if let Err(e) = inject::send_ctrl_c() {
        emit_status(&app, "error", Some(format!("Ctrl+C failed: {e:#}")));
        notify(&app, "Bulbul polish failed", &format!("{e:#}"));
        restore_clipboard(original);
        return;
    }

    // Give the foreground app a moment to populate the clipboard.
    tokio::time::sleep(Duration::from_millis(180)).await;

    let selected = match arboard::Clipboard::new().and_then(|mut c| c.get_text()) {
        Ok(s) => s,
        Err(_) => String::new(),
    };

    if selected.trim().is_empty() {
        emit_status(
            &app,
            "error",
            Some("No text selected — highlight something first.".into()),
        );
        notify(&app, "Bulbul polish", "No text selected — highlight some text and try again.");
        restore_clipboard(original);
        return;
    }

    emit_status(&app, "processing", None);
    let polished = match groq::polish(&cfg, &selected).await {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("polish failed: {e:#}");
            emit_status(&app, "error", Some(format!("{e:#}")));
            notify(&app, "Bulbul polish failed", &format!("{e:#}"));
            restore_clipboard(original);
            return;
        }
    };

    if polished.trim().is_empty() {
        emit_status(&app, "error", Some("Polish returned empty text.".into()));
        restore_clipboard(original);
        return;
    }

    let db = app.state::<AppState>().db.clone();
    let (final_text, dict_hits) = db::apply_substitutions(&db, &polished);
    if !dict_hits.is_empty() {
        let _ = db::bump_dictionary_hits(&db, &dict_hits);
    }

    emit_status(&app, "injecting", None);
    if let Err(e) = inject::inject_text(&final_text) {
        tracing::error!("polish inject failed: {e:#}");
        emit_status(&app, "error", Some(format!("Inject: {e:#}")));
        notify(&app, "Bulbul polish failed", &format!("{e:#}"));
        restore_clipboard(original);
        return;
    }

    emit_status(&app, "done", Some(final_text));
    // inject_text already attempts to restore the previous clipboard; we
    // simulated our own Ctrl+C so additionally schedule a restore after the
    // paste settles.
    tokio::time::sleep(Duration::from_millis(250)).await;
    restore_clipboard(original);
}

fn restore_clipboard(original: Option<String>) {
    if let Some(orig) = original {
        if let Ok(mut c) = arboard::Clipboard::new() {
            let _ = c.set_text(orig);
        }
    }
}

async fn process_pipeline(
    app: AppHandle,
    cfg: Config,
    wav: Vec<u8>,
    meta: PendingDictation,
    duration_ms: u64,
) {
    // Hand the dictionary's canonical spellings to Whisper as a prompt hint —
    // biases the STT toward the user's preferred forms (e.g. "Groq", "iOS").
    let db = app.state::<AppState>().db.clone();
    let vocabulary: Vec<String> = db::list_dictionary(&db)
        .unwrap_or_default()
        .into_iter()
        .map(|e| e.to_word)
        .collect();

    let transcript = match groq::transcribe(&cfg, wav, &vocabulary).await {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("STT failed: {e:#}");
            emit_status(&app, "error", Some(format!("STT: {e:#}")));
            notify(&app, "Bulbul transcription failed", &format!("{e:#}"));
            return;
        }
    };
    tracing::debug!("raw transcript: {transcript}");
    if transcript.trim().is_empty() || groq::is_likely_hallucination(&transcript) {
        tracing::info!("dropping likely-hallucinated transcript: {transcript:?}");
        emit_status(&app, "idle", Some("No speech detected.".into()));
        return;
    }

    let cleaned = match groq::cleanup(&cfg, &transcript).await {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!("cleanup failed, falling back to raw: {e:#}");
            transcript.clone()
        }
    };
    tracing::debug!("cleaned: {cleaned}");

    let (final_text, dict_hits) = db::apply_substitutions(&db, &cleaned);
    if !dict_hits.is_empty() {
        tracing::debug!("dictionary applied {} fix(es)", dict_hits.iter().map(|(_, c)| c).sum::<i64>());
        let _ = db::bump_dictionary_hits(&db, &dict_hits);
    }

    emit_status(&app, "injecting", None);
    if let Err(e) = inject::inject_text(&final_text) {
        tracing::error!("inject failed: {e:#}");
        emit_status(&app, "error", Some(format!("Inject: {e:#}")));
        notify(&app, "Bulbul inject failed", &format!("{e:#}"));
        return;
    }

    // Log this dictation to the activity store. Best-effort — failures here
    // never block injection or surface to the user.
    if let Err(e) = db::log_dictation(
        &db,
        db::LogEntry {
            raw_text: transcript,
            cleaned_text: final_text.clone(),
            mode: meta.mode,
            language: meta.language,
            foreground_app: meta.foreground_app,
            duration_ms,
        },
    ) {
        tracing::warn!("failed to log dictation: {e:#}");
    }

    emit_status(&app, "done", Some(final_text));
}
