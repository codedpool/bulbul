mod audio;
mod config;
mod groq;
mod hotkey;
mod inject;

use crate::audio::Recorder;
use crate::config::Config;
use crate::hotkey::{HotkeyEvent, ParsedHotkey};
use parking_lot::Mutex;
use serde::Serialize;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tauri::{
    image::Image,
    menu::{Menu, MenuItem},
    tray::{TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, LogicalPosition, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent,
};
use tauri_plugin_notification::NotificationExt;

const OVERLAY_WIDTH: f64 = 240.0;
const OVERLAY_HEIGHT: f64 = 56.0;
// Gap between the pill and the top of the taskbar / work area.
const OVERLAY_BOTTOM_MARGIN: f64 = 4.0;

pub struct AppState {
    config: Arc<Mutex<Config>>,
    hotkey: Arc<Mutex<ParsedHotkey>>,
    icons: Arc<IconVariants>,
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
    let (prev_has_key, prev_hotkey) = {
        let cfg = state.config.lock();
        (cfg.has_api_key(), cfg.hotkey.clone())
    };
    config::save(&new_cfg).map_err(|e| format!("{e:#}"))?;
    let next_has_key = new_cfg.has_api_key();
    let next_hotkey = new_cfg.hotkey.clone();
    *state.config.lock() = new_cfg;

    if prev_has_key != next_has_key {
        update_tray_icon(&app, next_has_key);
    }
    if prev_hotkey != next_hotkey {
        *state.hotkey.lock() = ParsedHotkey::parse(&next_hotkey);
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
    let parsed_hotkey = ParsedHotkey::parse(&initial_config.hotkey);
    let hotkey_mutex = Arc::new(Mutex::new(parsed_hotkey.clone()));

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            get_config,
            save_config,
            validate_api_key,
            check_for_updates,
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

            handle.manage(AppState {
                config: Arc::new(Mutex::new(initial_config)),
                hotkey: hotkey_mutex.clone(),
                icons,
            });

            setup_tray(&handle, has_key_on_boot)?;
            setup_overlay_window(&handle)?;

            if let Some(window) = handle.get_webview_window("main") {
                if !has_key_on_boot {
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

fn spawn_orchestrator(handle: AppHandle, hotkey: Arc<Mutex<ParsedHotkey>>) {
    let initial = hotkey.lock().clone();
    let (live_hotkey, rx) = hotkey::spawn_listener(initial);

    // Keep listener's hotkey in sync with the AppState hotkey mutex.
    {
        let live_hotkey = live_hotkey.clone();
        let hotkey = hotkey.clone();
        thread::spawn(move || loop {
            thread::sleep(Duration::from_millis(500));
            let current = hotkey.lock().clone();
            let mut listener = live_hotkey.lock();
            if *listener != current {
                *listener = current;
            }
        });
    }

    thread::spawn(move || {
        let mut active_recorder: Option<Recorder> = None;
        for evt in rx {
            match evt {
                HotkeyEvent::Pressed => {
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
                            active_recorder = Some(rec);
                        }
                        Err(e) => {
                            tracing::error!("recorder start failed: {e:#}");
                            emit_status(&handle, "error", Some(format!("{e:#}")));
                            notify(&handle, "Bulbul mic error", &format!("{e:#}"));
                        }
                    }
                }
                HotkeyEvent::Released => {
                    let Some(rec) = active_recorder.take() else {
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
                    let wav = match rec.finish() {
                        Ok(b) => b,
                        Err(e) => {
                            tracing::error!("encoding WAV failed: {e:#}");
                            emit_status(&handle, "error", Some(format!("{e:#}")));
                            notify(&handle, "Bulbul audio error", &format!("{e:#}"));
                            continue;
                        }
                    };
                    emit_status(&handle, "processing", None);
                    let handle_for_task = handle.clone();
                    tauri::async_runtime::spawn(async move {
                        process_pipeline(handle_for_task, cfg, wav).await;
                    });
                }
            }
        }
    });
}

async fn process_pipeline(app: AppHandle, cfg: Config, wav: Vec<u8>) {
    let transcript = match groq::transcribe(&cfg, wav).await {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("STT failed: {e:#}");
            emit_status(&app, "error", Some(format!("STT: {e:#}")));
            notify(&app, "Bulbul transcription failed", &format!("{e:#}"));
            return;
        }
    };
    tracing::debug!("raw transcript: {transcript}");
    if transcript.trim().is_empty() {
        emit_status(&app, "idle", Some("No speech detected.".into()));
        return;
    }

    let cleaned = match groq::cleanup(&cfg, &transcript).await {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!("cleanup failed, falling back to raw: {e:#}");
            transcript
        }
    };
    tracing::debug!("cleaned: {cleaned}");

    emit_status(&app, "injecting", None);
    if let Err(e) = inject::inject_text(&cleaned) {
        tracing::error!("inject failed: {e:#}");
        emit_status(&app, "error", Some(format!("Inject: {e:#}")));
        notify(&app, "Bulbul inject failed", &format!("{e:#}"));
        return;
    }
    emit_status(&app, "done", Some(cleaned));
}
