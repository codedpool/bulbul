// Bulbul on Android (and eventually iOS).
//
// Phase 1: Tauri app that boots, opens the WebView, loads dist/index.html.
// Phase 2: stubbed the mount-time invoke commands so the loading screen
// advances to actual UI.
// Phase 3 (this): stub every command the React shell calls when the
// user navigates between dashboard pages, so each view renders its
// empty state cleanly instead of erroring in the console. Read
// commands return empty Vecs / zeroed shapes; mutating commands are
// no-ops; expensive commands (transforms, Groq round-trips) return an
// "not supported on Android yet" error that the UI already handles.
//
// Every stub is replaced with a real implementation as the
// corresponding feature lands on mobile — most of that work is in the
// floating-bubble overlay + AccessibilityService path, not the
// dashboard, so these stubs may stay in place for a while.
//
// What's deliberately NOT here:
//   - cpal-based audio capture (replaced by Kotlin AudioRecord + JNI)
//   - tauri-plugin-global-shortcut (no global hotkeys on Android)
//   - tauri-plugin-single-instance (Android process model handles this)
//   - tray icon / overlay window / hover-watcher (different on mobile)
//   - hotkey orchestrator, dictation pipeline (pulled in once audio
//     bridge is in place)

mod config;

use config::Config;
use serde_json::{json, Value};

// ---------- Startup commands (App.jsx useEffect on mount) ----------

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

// ---------- Home + Insights (read-only stats) ----------

/// Matches `db::HomeStats` shape — zeroed out until the on-device
/// dictation history lands.
#[tauri::command]
fn get_home_stats() -> Value {
    json!({
        "total_words": 0,
        "total_dictations": 0,
        "total_fixes": 0,
        "wpm": 0.0,
        "day_streak": 0,
    })
}

#[tauri::command]
fn get_recent_dictations(_limit: u32, _offset: u32) -> Vec<Value> {
    Vec::new()
}

/// Matches `db::UsageStats` shape. All-zero defaults so the React
/// Insights page renders the empty state without a console error.
#[tauri::command]
fn get_insights_usage() -> Value {
    json!({
        "wpm": 0.0,
        "wpm_percentile": 0.0,
        "total_words": 0,
        "words_this_month": 0,
        "words_last_month": 0,
        "mom_change_pct": null,
        "total_fixes": 0,
        "ai_fixes": 0,
        "dictionary_fixes": 0,
        "day_streak": 0,
        "longest_streak": 0,
        "total_apps_used": 0,
        "app_usage": [],
        "heatmap": [],
    })
}

/// Matches `db::VoiceStats` shape.
#[tauri::command]
fn get_voice_stats() -> Value {
    json!({
        "most_used_word": null,
        "most_corrected_word": null,
        "catchphrase": null,
        "peak_day_name": null,
        "peak_hour_label": null,
        "peak_app": null,
        "peak_app_category": null,
        "voice_narrative": null,
        "peak_narrative": null,
        "last_generated_at": null,
        "words_since_last_gen": 0,
        "min_words_to_refresh": 200,
        "total_words": 0,
        "has_api_key": false,
    })
}

#[tauri::command]
fn refresh_voice_narrative() -> Result<Value, String> {
    Err("Voice narrative is not yet supported on Android.".to_string())
}

// ---------- Dictionary + corrections ----------

#[tauri::command]
fn list_dictionary() -> Vec<Value> {
    Vec::new()
}

#[tauri::command]
fn correction_suggestions() -> Vec<Value> {
    Vec::new()
}

#[tauri::command]
fn list_corrections(_limit: u32) -> Vec<Value> {
    Vec::new()
}

#[tauri::command]
fn add_dictionary_entry(
    _from_word: String,
    _to_word: String,
    _case_sensitive: bool,
) -> Result<(), String> {
    Ok(())
}

#[tauri::command]
fn update_dictionary_entry(
    _id: i64,
    _from_word: String,
    _to_word: String,
    _case_sensitive: bool,
) -> Result<(), String> {
    Ok(())
}

#[tauri::command]
fn delete_dictionary_entry(_id: i64) -> Result<(), String> {
    Ok(())
}

#[tauri::command]
fn dismiss_correction_suggestion(_from_word: String) -> Result<(), String> {
    Ok(())
}

// ---------- Snippets ----------

#[tauri::command]
fn list_snippets() -> Vec<Value> {
    Vec::new()
}

#[tauri::command]
fn add_snippet(_trigger: String, _expansion: String) -> Result<(), String> {
    Ok(())
}

#[tauri::command]
fn update_snippet(_id: i64, _trigger: String, _expansion: String) -> Result<(), String> {
    Ok(())
}

#[tauri::command]
fn delete_snippet(_id: i64) -> Result<(), String> {
    Ok(())
}

// ---------- Transforms ----------

#[tauri::command]
fn list_transforms() -> Vec<Value> {
    Vec::new()
}

#[tauri::command]
fn list_transform_slot_statuses() -> Vec<Value> {
    Vec::new()
}

#[tauri::command]
fn add_transform(
    _name: String,
    _prompt: String,
    _is_default: bool,
    _slot: Option<u8>,
) -> Result<(), String> {
    Ok(())
}

#[tauri::command]
fn update_transform(
    _id: i64,
    _name: String,
    _prompt: String,
    _is_default: bool,
    _slot: Option<u8>,
) -> Result<(), String> {
    Ok(())
}

#[tauri::command]
fn delete_transform(_id: i64) -> Result<(), String> {
    Ok(())
}

#[tauri::command]
fn set_default_transform(_id: i64) -> Result<(), String> {
    Ok(())
}

#[tauri::command]
fn reset_transforms() -> Result<(), String> {
    Ok(())
}

#[tauri::command]
fn run_transform_on_text(_id: i64, _text: String) -> Result<String, String> {
    Err("Running transforms on text is not yet supported on Android.".to_string())
}

// ---------- Notes / scratchpad ----------

#[tauri::command]
fn list_notes() -> Vec<Value> {
    Vec::new()
}

#[tauri::command]
fn create_note(_title: String, _body: String) -> Result<Value, String> {
    Err("The scratchpad is not yet supported on Android.".to_string())
}

#[tauri::command]
fn update_note(_id: i64, _title: String, _body: String) -> Result<(), String> {
    Ok(())
}

#[tauri::command]
fn delete_note(_id: i64) -> Result<(), String> {
    Ok(())
}

// ---------- Settings + updater ----------

#[tauri::command]
async fn validate_api_key(_api_key: String) -> Result<(), String> {
    Err("API-key validation is not yet supported on Android.".to_string())
}

#[tauri::command]
async fn check_for_updates() -> Result<Option<String>, String> {
    Ok(None)
}

// ---------- Overlay / scratchpad windows (desktop-only concepts) ----------

#[tauri::command]
fn set_overlay_height(_height: u32) -> Result<(), String> {
    Ok(())
}

#[tauri::command]
fn open_scratchpad() -> Result<(), String> {
    Err("Scratchpad window is not available on Android.".to_string())
}

#[tauri::command]
fn complete_onboarding() -> Result<(), String> {
    Ok(())
}

// ---------- Entry point ----------

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(tauri::generate_handler![
            // Startup
            get_config,
            save_config,
            get_autostart,
            set_autostart,
            get_staged_update_version,
            install_staged_update,
            set_tray_visible,
            // Home + Insights
            get_home_stats,
            get_recent_dictations,
            get_insights_usage,
            get_voice_stats,
            refresh_voice_narrative,
            // Dictionary + corrections
            list_dictionary,
            correction_suggestions,
            list_corrections,
            add_dictionary_entry,
            update_dictionary_entry,
            delete_dictionary_entry,
            dismiss_correction_suggestion,
            // Snippets
            list_snippets,
            add_snippet,
            update_snippet,
            delete_snippet,
            // Transforms
            list_transforms,
            list_transform_slot_statuses,
            add_transform,
            update_transform,
            delete_transform,
            set_default_transform,
            reset_transforms,
            run_transform_on_text,
            // Notes / scratchpad
            list_notes,
            create_note,
            update_note,
            delete_note,
            // Settings + updater
            validate_api_key,
            check_for_updates,
            // Overlay / scratchpad windows
            set_overlay_height,
            open_scratchpad,
            complete_onboarding,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Bulbul on mobile");
}
