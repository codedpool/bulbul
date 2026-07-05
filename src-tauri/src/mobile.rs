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
use tauri::Manager;

/// Where Bulbul keeps mobile config on disk. Tauri's app_data_dir on
/// Android resolves to Context.getFilesDir() (i.e.
/// /data/data/com.bulbul.app/files/), so reading the same path from
/// Kotlin via `filesDir` is the bridge that lets BulbulForegroundService
/// pick up the Groq API key the React Settings UI saves here.
const MOBILE_CONFIG_FILE: &str = "config.json";

fn mobile_config_defaults(mut cfg: Config) -> Config {
    cfg.onboarding_completed = true;
    cfg.privacy_acknowledged = true;
    cfg
}

// ---------- Startup commands (App.jsx useEffect on mount) ----------

/// Reads the persisted config from app-private storage. Falls back to
/// a mobile-flavoured default (onboarding + privacy acknowledged
/// pre-flipped) so the React shell skips the desktop-shaped wizard
/// and renders the main dashboard the first time the user opens the
/// app on a fresh install.
#[tauri::command]
fn get_config(app: tauri::AppHandle) -> Config {
    let cfg = read_config(&app).unwrap_or_default();
    mobile_config_defaults(cfg)
}

/// Writes the config as JSON to <app_data_dir>/config.json. The Kotlin
/// foreground service reads the same file to pick up the Groq API
/// key — no JNI needed because both sides agree on the path
/// (Android's Context.getFilesDir() == Tauri's app_data_dir on this
/// platform).
#[tauri::command]
fn save_config(app: tauri::AppHandle, new_cfg: Config) -> Result<(), String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(MOBILE_CONFIG_FILE);
    let json = serde_json::to_string_pretty(&new_cfg).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())
}

fn read_config(app: &tauri::AppHandle) -> Option<Config> {
    let dir = app.path().app_data_dir().ok()?;
    let path = dir.join(MOBILE_CONFIG_FILE);
    if !path.exists() {
        return None;
    }
    let text = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str::<Config>(&text).ok()
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

/// One JSON object per line, appended by the Kotlin foreground service
/// after each successful transcription (see recordHistory). File-as-IPC,
/// same pattern as config.json.
const HISTORY_FILE: &str = "history.jsonl";

fn history_rows(app: &tauri::AppHandle) -> Vec<Value> {
    let Ok(dir) = app.path().app_data_dir() else { return Vec::new() };
    let Ok(text) = std::fs::read_to_string(dir.join(HISTORY_FILE)) else { return Vec::new() };
    text.lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .collect()
}

#[tauri::command]
fn get_home_stats(app: tauri::AppHandle) -> Value {
    let rows = history_rows(&app);
    let total_words: u64 = rows.iter().map(|r| r["word_count"].as_u64().unwrap_or(0)).sum();
    let total_ms: u64 = rows.iter().map(|r| r["duration_ms"].as_u64().unwrap_or(0)).sum();
    let wpm = if total_ms > 0 {
        total_words as f64 / (total_ms as f64 / 60_000.0)
    } else {
        0.0
    };
    // Streak: consecutive UTC days with at least one dictation, counting
    // back from today. Good enough without pulling in a timezone crate.
    let mut days: Vec<i64> = rows
        .iter()
        .filter_map(|r| r["ts"].as_i64())
        .map(|ts| ts / 86_400)
        .collect();
    days.sort_unstable();
    days.dedup();
    let today = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64 / 86_400)
        .unwrap_or(0);
    let mut streak = 0i64;
    while days.binary_search(&(today - streak)).is_ok() {
        streak += 1;
    }
    json!({
        "total_words": total_words,
        "total_dictations": rows.len(),
        "total_fixes": 0,
        "wpm": wpm,
        "day_streak": streak,
    })
}

#[tauri::command]
fn get_recent_dictations(app: tauri::AppHandle, limit: u32, offset: u32) -> Vec<Value> {
    let rows = history_rows(&app);
    rows.iter()
        .rev() // newest first
        .skip(offset as usize)
        .take(limit as usize)
        .enumerate()
        .map(|(i, r)| {
            json!({
                "id": (offset as usize + i) as i64,
                "ts": r["ts"],
                "cleaned_text": r["cleaned_text"],
                "foreground_app": r.get("foreground_app").cloned().unwrap_or(Value::Null),
                "mode": r.get("mode").cloned().unwrap_or(json!("clean")),
                "word_count": r["word_count"],
            })
        })
        .collect()
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

/// Pings Groq's `/v1/models` with the user-provided key. 200 → key
/// works. 401 → key is wrong. Anything else → surface the body so the
/// user can see what Groq said (rate-limit, account suspended, etc.).
/// Mirrors `groq::validate_key` on desktop; kept inline here so the
/// mobile build doesn't pull in the entire desktop `groq` module
/// (which depends on `hound`, `tokio` retry helpers, and the shared
/// client cache that has different requirements on mobile).
#[tauri::command]
async fn validate_api_key(api_key: String) -> Result<(), String> {
    let key = api_key.trim();
    if key.is_empty() {
        return Err("API key is empty.".to_string());
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| format!("HTTP client init failed: {e}"))?;
    let resp = client
        .get("https://api.groq.com/openai/v1/models")
        .bearer_auth(key)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;
    if resp.status().is_success() {
        Ok(())
    } else {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        Err(format!("Groq rejected key ({status}): {body}"))
    }
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
    // rustls 0.23 (transitively pulled in by reqwest's rustls-tls
    // feature) refuses to operate until a CryptoProvider is installed
    // in the process-default registry. On Windows + macOS reqwest
    // falls back to a native TLS backend so this never trips, but on
    // Android there is no native backend — the first reqwest call
    // (which Tauri's IPC layer makes internally on the first
    // invoke) panics with "No provider set" and aborts the process.
    // Install ring early; .ok() so a re-init from a hot-reload or a
    // duplicate caller doesn't itself panic.
    let _ = rustls::crypto::ring::default_provider().install_default();

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
