// Bulbul on Android (and eventually iOS).
//
// Phase 1: Tauri app that boots, opens the WebView, loads dist/index.html.
// Phase 2 (this): stub the invoke commands the React UI awaits on mount
// so the loading screen can advance to actual UI. Most stubs return
// defaults or no-ops — every command is replaced with a real
// implementation as the corresponding feature lands on mobile.
//
// What's deliberately NOT here yet:
//   - cpal-based audio capture (replaced by Kotlin AudioRecord + JNI)
//   - tauri-plugin-global-shortcut (no global hotkeys on Android)
//   - tauri-plugin-single-instance (Android process model handles this)
//   - tray icon / overlay window / hover-watcher (different on mobile)
//   - hotkey orchestrator, dictation pipeline (pulled in once audio
//     bridge is in place)

mod config;
mod db;
mod groq;
mod telemetry;

use config::Config;

/// Returns a Bulbul config with `onboarding_completed = true` so the
/// React shell skips the desktop-shaped wizard and renders the main
/// dashboard. The Android-flavored onboarding (mic + accessibility +
/// overlay permissions) is a separate flow added once the
/// AccessibilityService and overlay bubble land.
#[tauri::command]
fn get_config() -> Config {
    let mut cfg = Config::default();
    cfg.onboarding_completed = true;
    cfg.privacy_acknowledged = true;
    cfg
}

/// Persisting config on Android is a no-op for now. Once mobile storage
/// is wired (Android SharedPreferences or app-private file), this gets
/// replaced with a real write.
#[tauri::command]
fn save_config(_new_cfg: Config) -> Result<(), String> {
    Ok(())
}

/// Autostart on Android is governed by the BOOT_COMPLETED broadcast,
/// not the Tauri autostart plugin. Always returns false for now.
#[tauri::command]
fn get_autostart() -> Result<bool, String> {
    Ok(false)
}

#[tauri::command]
fn set_autostart(_enabled: bool) -> Result<(), String> {
    Ok(())
}

/// No in-app updater on Android yet — distribution is sideloaded APK.
/// Returning None means the React updater banner stays hidden.
#[tauri::command]
fn get_staged_update_version() -> Option<String> {
    None
}

#[tauri::command]
fn install_staged_update() -> Result<(), String> {
    Err("Updates are not yet supported on Android — reinstall the APK manually.".to_string())
}

/// Tray doesn't exist on Android — there's a foreground service
/// notification instead (added with the floating bubble work).
#[tauri::command]
fn set_tray_visible(_visible: bool) -> Result<(), String> {
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(tauri::generate_handler![
            get_config,
            save_config,
            get_autostart,
            set_autostart,
            get_staged_update_version,
            install_staged_update,
            set_tray_visible,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Bulbul on mobile");
}
