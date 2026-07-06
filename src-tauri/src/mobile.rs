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
    // Privacy is acknowledged implicitly on mobile (no separate privacy
    // modal). Onboarding is deliberately NOT forced here: the React wizard
    // runs once on first launch — after the native permission screen —
    // and persists its completion via complete_onboarding.
    cfg.privacy_acknowledged = true;
    cfg
}

// ---------- Startup commands (App.jsx useEffect on mount) ----------

/// Reads the persisted config from app-private storage, applying the
/// mobile defaults (privacy acknowledged; onboarding driven by the real
/// persisted flag so the first-launch wizard runs exactly once).
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
    let total_fixes: u64 = rows.iter().map(|r| r["fix_count"].as_u64().unwrap_or(0)).sum();
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
        "total_fixes": total_fixes,
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

// ---------- File-backed stores (transforms, dictionary) ----------
//
// Desktop keeps these in SQLite; the mobile build doesn't pull in rusqlite,
// so each list is a JSON array on disk next to config.json / history.jsonl
// (the same app_data_dir file-as-IPC pattern). Seeded from defaults on
// first run — but only when the file is ABSENT, so deleting every entry
// stays deleted instead of respawning the seeds.

const TRANSFORMS_FILE: &str = "transforms.json";
const DICTIONARY_FILE: &str = "dictionary.json";

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn read_json_array(app: &tauri::AppHandle, file: &str) -> Vec<Value> {
    let Ok(dir) = app.path().app_data_dir() else { return Vec::new() };
    match std::fs::read_to_string(dir.join(file)) {
        Ok(text) => serde_json::from_str::<Vec<Value>>(&text).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

fn write_json_array(app: &tauri::AppHandle, file: &str, rows: &[Value]) -> Result<(), String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(rows).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(file), json).map_err(|e| e.to_string())
}

fn next_id(rows: &[Value]) -> i64 {
    rows.iter().filter_map(|r| r["id"].as_i64()).max().unwrap_or(0) + 1
}

const VOICE_FILE: &str = "voice.json";

fn read_json_object(app: &tauri::AppHandle, file: &str) -> Option<Value> {
    let dir = app.path().app_data_dir().ok()?;
    let text = std::fs::read_to_string(dir.join(file)).ok()?;
    serde_json::from_str::<Value>(&text).ok()
}

fn write_json_object(app: &tauri::AppHandle, file: &str, obj: &Value) -> Result<(), String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(obj).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(file), json).map_err(|e| e.to_string())
}

// Overlay runtime state shared with the Kotlin side (file-as-IPC, same as
// history/dictionary). Kept OUT of config.json so it isn't clobbered by the
// React settings save, which overwrites the whole config object. Holds the
// snooze deadline: `snoozed_until` is a unix-seconds timestamp (0 = active).
const OVERLAY_FILE: &str = "overlay.json";

/// Seconds-since-epoch until which the overlay bubble is snoozed (0/none if
/// not snoozed). The Kotlin service writes this when the user drops the bubble
/// on the snooze target; the Settings page reads it to show the state.
#[tauri::command]
fn get_overlay_snoozed_until(app: tauri::AppHandle) -> i64 {
    read_json_object(&app, OVERLAY_FILE)
        .and_then(|v| v.get("snoozed_until").and_then(Value::as_i64))
        .unwrap_or(0)
}

/// Resume the overlay immediately — clears any active snooze so the bubble
/// reappears the next time the user focuses a text field. Wired to the
/// "Resume now" button in Settings → Overlay.
#[tauri::command]
fn resume_overlay(app: tauri::AppHandle) -> Result<(), String> {
    write_json_object(&app, OVERLAY_FILE, &json!({ "snoozed_until": 0 }))
}

/// One-shot Groq chat completion. Mirrors `groq::chat` on desktop but kept
/// inline so the mobile build doesn't pull the whole desktop groq module.
async fn groq_chat(
    api_key: &str,
    model: &str,
    system: &str,
    user: &str,
    temperature: f32,
) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("HTTP client init failed: {e}"))?;
    let body = json!({
        "model": model,
        "temperature": temperature,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user },
        ],
    });
    let resp = client
        .post("https://api.groq.com/openai/v1/chat/completions")
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("Groq error ({status}): {text}"));
    }
    let v: Value = resp.json().await.map_err(|e| format!("parse error: {e}"))?;
    v["choices"][0]["message"]["content"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "Groq returned an empty response".to_string())
}

/// Read a store, or seed + persist it if the file doesn't exist yet.
/// Keyed on file existence (not emptiness) so an emptied list stays empty.
fn load_store(app: &tauri::AppHandle, file: &str, seed: impl FnOnce() -> Vec<Value>) -> Vec<Value> {
    let absent = app
        .path()
        .app_data_dir()
        .ok()
        .map(|d| !d.join(file).exists())
        .unwrap_or(true);
    if absent {
        let seeded = seed();
        let _ = write_json_array(app, file, &seeded);
        seeded
    } else {
        read_json_array(app, file)
    }
}

/// Days-since-epoch → (year, month, day). Howard Hinnant's civil-from-days
/// algorithm — lets us bucket history by calendar month/day without pulling
/// a date crate into the mobile build.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Same coarse WPM → percentile buckets the desktop uses (db.rs), so the
/// gauge reads identically across platforms.
fn wpm_percentile_estimate(wpm: f64) -> f64 {
    if wpm >= 200.0 { 0.1 }
    else if wpm >= 150.0 { 1.0 }
    else if wpm >= 120.0 { 5.0 }
    else if wpm >= 100.0 { 20.0 }
    else if wpm >= 80.0 { 50.0 }
    else if wpm >= 60.0 { 75.0 }
    else { 90.0 }
}

/// Real usage stats computed from history.jsonl (the same file Home reads).
/// Matches `db::UsageStats` shape. Fix counts stay 0 — mobile doesn't track
/// per-dictation fixes yet — and app_usage stays empty (the React Insights
/// page hides the "Desktop usage" panel on Android).
#[tauri::command]
fn get_insights_usage(app: tauri::AppHandle) -> Value {
    let rows = history_rows(&app);
    let now = now_secs();
    let today = now.div_euclid(86_400);

    let total_words: i64 = rows.iter().map(|r| r["word_count"].as_i64().unwrap_or(0)).sum();
    // Every mobile fix is a dictionary substitution (no AI cleanup pass on
    // mobile yet), so total_fixes == dictionary_fixes and ai_fixes stays 0.
    let total_fixes: i64 = rows.iter().map(|r| r["fix_count"].as_i64().unwrap_or(0)).sum();

    // WPM over the last 7 days — same window Home uses.
    let cutoff = now - 7 * 86_400;
    let (mut words_7d, mut ms_7d) = (0i64, 0i64);
    for r in &rows {
        if r["ts"].as_i64().unwrap_or(0) >= cutoff {
            words_7d += r["word_count"].as_i64().unwrap_or(0);
            ms_7d += r["duration_ms"].as_i64().unwrap_or(0);
        }
    }
    let wpm = if ms_7d > 0 { words_7d as f64 / (ms_7d as f64 / 60_000.0) } else { 0.0 };

    // Month-over-month words, by real calendar month.
    let (cy, cm, _) = civil_from_days(today);
    let (py, pm) = if cm == 1 { (cy - 1, 12) } else { (cy, cm - 1) };
    let (mut words_this_month, mut words_last_month) = (0i64, 0i64);
    for r in &rows {
        let (y, m, _) = civil_from_days(r["ts"].as_i64().unwrap_or(0).div_euclid(86_400));
        let w = r["word_count"].as_i64().unwrap_or(0);
        if y == cy && m == cm {
            words_this_month += w;
        } else if y == py && m == pm {
            words_last_month += w;
        }
    }
    let mom_change_pct = if words_last_month > 0 {
        Some((words_this_month - words_last_month) as f64 / words_last_month as f64 * 100.0)
    } else if words_this_month > 0 {
        None
    } else {
        Some(0.0)
    };

    // Streaks — distinct UTC days with at least one dictation.
    let mut days: Vec<i64> = rows
        .iter()
        .filter_map(|r| r["ts"].as_i64())
        .map(|ts| ts.div_euclid(86_400))
        .collect();
    days.sort_unstable();
    days.dedup();
    let mut day_streak = 0i64;
    while days.binary_search(&(today - day_streak)).is_ok() {
        day_streak += 1;
    }
    let (mut longest_streak, mut cur, mut prev) = (0i64, 0i64, None::<i64>);
    for &d in &days {
        cur = if prev == Some(d - 1) { cur + 1 } else { 1 };
        if cur > longest_streak {
            longest_streak = cur;
        }
        prev = Some(d);
    }

    // Heatmap: last 90 days as {date, count}. React keys by date string, so
    // order doesn't matter.
    let mut counts: std::collections::HashMap<i64, i64> = std::collections::HashMap::new();
    for r in &rows {
        *counts
            .entry(r["ts"].as_i64().unwrap_or(0).div_euclid(86_400))
            .or_insert(0) += 1;
    }
    let heatmap: Vec<Value> = (0..90)
        .map(|i| {
            let d = today - i;
            let (y, m, day) = civil_from_days(d);
            json!({
                "date": format!("{:04}-{:02}-{:02}", y, m, day),
                "count": counts.get(&d).copied().unwrap_or(0),
            })
        })
        .collect();

    json!({
        "wpm": wpm,
        "wpm_percentile": wpm_percentile_estimate(wpm),
        "total_words": total_words,
        "words_this_month": words_this_month,
        "words_last_month": words_last_month,
        "mom_change_pct": mom_change_pct,
        "total_fixes": total_fixes,
        "ai_fixes": 0,
        "dictionary_fixes": total_fixes,
        "day_streak": day_streak,
        "longest_streak": longest_streak,
        "total_apps_used": 0,
        "app_usage": [],
        "heatmap": heatmap,
    })
}

/// Matches `db::VoiceStats` shape. The narrative fields stay null (the
/// Groq-generated voice profile isn't wired on mobile yet), but total_words
/// and has_api_key come from real state so the "Your Voice" tab shows the
/// correct "dictate N more words to unlock" progress instead of a dead 0.
#[tauri::command]
fn voice_stats_value(app: &tauri::AppHandle) -> Value {
    let total_words: i64 = history_rows(app)
        .iter()
        .map(|r| r["word_count"].as_i64().unwrap_or(0))
        .sum();
    let has_api_key = read_config(app).map(|c| c.has_api_key()).unwrap_or(false);

    let voice = read_json_object(app, VOICE_FILE);
    let field = |k: &str| {
        voice
            .as_ref()
            .and_then(|v| v[k].as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
    };
    let last_generated_at = voice.as_ref().and_then(|v| v["last_generated_at"].as_i64());
    let words_at_gen = voice.as_ref().and_then(|v| v["words_at_gen"].as_i64()).unwrap_or(0);
    let words_since = (total_words - words_at_gen).max(0);

    json!({
        "most_used_word": null,
        "most_corrected_word": null,
        "catchphrase": null,
        "peak_day_name": null,
        "peak_hour_label": null,
        "peak_app": null,
        "peak_app_category": null,
        "voice_narrative": field("voice_narrative"),
        "peak_narrative": field("peak_narrative"),
        "last_generated_at": last_generated_at,
        "words_since_last_gen": words_since,
        "min_words_to_refresh": 200,
        "total_words": total_words,
        "has_api_key": has_api_key,
    })
}

#[tauri::command]
fn get_voice_stats(app: tauri::AppHandle) -> Value {
    voice_stats_value(&app)
}

const VOICE_PROFILE_SYSTEM_PROMPT: &str = "You analyze a person's voice-dictation history to describe their communication style. \
From the dictation samples, write a warm, specific 2-3 sentence \"voice profile\" describing how this person comes across when they dictate — their tone, the kinds of things they say, and any recurring quirks. Address them as \"you\". \
Then write one short sentence (\"peak blurb\") about their dictation habit or rhythm. \
Do not invent facts not supported by the samples. \
Return ONLY a JSON object, no preamble and no markdown fences:\n\
{\"voice_profile\": \"...\", \"peak_blurb\": \"...\"}";

/// Generates the "Your Voice" narrative from the user's dictation history via
/// Groq, persists it to voice.json, and returns the refreshed VoiceStats.
#[tauri::command]
async fn refresh_voice_narrative(app: tauri::AppHandle) -> Result<Value, String> {
    let cfg = read_config(&app).unwrap_or_default();
    if !cfg.has_api_key() {
        return Err("Set your Groq API key in Settings first.".to_string());
    }
    let rows = history_rows(&app);
    let total_words: i64 = rows.iter().map(|r| r["word_count"].as_i64().unwrap_or(0)).sum();
    if total_words == 0 {
        return Err("Dictate something first to build a voice profile.".to_string());
    }

    // Most recent ~25 non-empty transcripts as the writing sample.
    let samples: String = rows
        .iter()
        .rev()
        .filter_map(|r| r["cleaned_text"].as_str())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .take(25)
        .collect::<Vec<_>>()
        .join("\n");
    let user = format!(
        "Quick stats:\nTotal words dictated: {total_words}. Total dictations: {}.\n\nDictation samples:\n{samples}",
        rows.len(),
    );

    let content = groq_chat(
        cfg.groq_api_key.trim(),
        &cfg.chat_model,
        VOICE_PROFILE_SYSTEM_PROMPT,
        &user,
        0.4,
    )
    .await?;

    // Strip any code fences the model added despite instructions.
    let trimmed = content
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    let parsed: Value = serde_json::from_str(trimmed)
        .map_err(|e| format!("Groq returned unparseable JSON: {e}"))?;

    let record = json!({
        "voice_narrative": parsed["voice_profile"].as_str().unwrap_or_default(),
        "peak_narrative": parsed["peak_blurb"].as_str().unwrap_or_default(),
        "last_generated_at": now_secs(),
        "words_at_gen": total_words,
    });
    write_json_object(&app, VOICE_FILE, &record)?;
    Ok(voice_stats_value(&app))
}

// ---------- Dictionary + corrections ----------

// Seeded on first run so the Dictionary page opens with useful entries
// instead of an empty list. Mirrors db.rs DEFAULT_DICTIONARY.
const DEFAULT_DICTIONARY: &[(&str, &str)] = &[
    ("groq", "Groq"),
    ("github", "GitHub"),
    ("gitlab", "GitLab"),
    ("vscode", "VS Code"),
    ("javascript", "JavaScript"),
    ("typescript", "TypeScript"),
    ("nodejs", "Node.js"),
    ("npm", "npm"),
    ("pnpm", "pnpm"),
    ("postgres", "Postgres"),
    ("sqlite", "SQLite"),
    ("ios", "iOS"),
    ("macos", "macOS"),
    ("api", "API"),
    ("ui", "UI"),
    ("ux", "UX"),
    ("css", "CSS"),
    ("html", "HTML"),
    ("json", "JSON"),
    ("ai", "AI"),
    ("llm", "LLM"),
    ("openai", "OpenAI"),
    ("anthropic", "Anthropic"),
];

fn seed_dictionary() -> Vec<Value> {
    DEFAULT_DICTIONARY
        .iter()
        .enumerate()
        .map(|(i, (from, to))| {
            json!({
                "id": i as i64 + 1,
                "from_word": from,
                "to_word": to,
                "case_sensitive": false,
                "hit_count": 0,
                "created_at": now_secs(),
            })
        })
        .collect()
}

fn load_dictionary(app: &tauri::AppHandle) -> Vec<Value> {
    load_store(app, DICTIONARY_FILE, seed_dictionary)
}

#[tauri::command]
fn list_dictionary(app: tauri::AppHandle) -> Vec<Value> {
    load_dictionary(&app)
}

#[tauri::command]
fn correction_suggestions() -> Vec<Value> {
    Vec::new()
}

#[tauri::command]
fn list_corrections(_limit: Option<u32>) -> Vec<Value> {
    // Correction tracking (detecting when the user hand-edits injected text)
    // needs post-injection edit monitoring that isn't built on mobile yet, so
    // this stays empty. Option<u32> so the arg-less invoke from the UI doesn't
    // error on a missing `limit` key.
    Vec::new()
}

#[tauri::command]
fn add_dictionary_entry(
    app: tauri::AppHandle,
    from_word: String,
    to_word: String,
    case_sensitive: bool,
) -> Result<(), String> {
    let from = from_word.trim();
    let to = to_word.trim();
    if from.is_empty() || to.is_empty() {
        return Err("Word cannot be empty.".into());
    }
    let mut rows = load_dictionary(&app);
    // Adding a word that already exists just updates its target, so the
    // list doesn't accumulate duplicate rows for the same trigger.
    if let Some(existing) = rows.iter_mut().find(|r| {
        r["from_word"].as_str().map(|s| s.eq_ignore_ascii_case(from)).unwrap_or(false)
    }) {
        existing["to_word"] = json!(to);
        existing["case_sensitive"] = json!(case_sensitive);
    } else {
        let id = next_id(&rows);
        rows.push(json!({
            "id": id,
            "from_word": from,
            "to_word": to,
            "case_sensitive": case_sensitive,
            "hit_count": 0,
            "created_at": now_secs(),
        }));
    }
    write_json_array(&app, DICTIONARY_FILE, &rows)
}

#[tauri::command]
fn update_dictionary_entry(
    app: tauri::AppHandle,
    id: i64,
    from_word: String,
    to_word: String,
    case_sensitive: bool,
) -> Result<(), String> {
    let from = from_word.trim();
    let to = to_word.trim();
    if from.is_empty() || to.is_empty() {
        return Err("Word cannot be empty.".into());
    }
    let mut rows = load_dictionary(&app);
    let mut found = false;
    for r in rows.iter_mut() {
        if r["id"].as_i64() == Some(id) {
            r["from_word"] = json!(from);
            r["to_word"] = json!(to);
            r["case_sensitive"] = json!(case_sensitive);
            found = true;
            break;
        }
    }
    if !found {
        return Err(format!("no dictionary entry with id {id}"));
    }
    write_json_array(&app, DICTIONARY_FILE, &rows)
}

#[tauri::command]
fn delete_dictionary_entry(app: tauri::AppHandle, id: i64) -> Result<(), String> {
    let mut rows = load_dictionary(&app);
    let before = rows.len();
    rows.retain(|r| r["id"].as_i64() != Some(id));
    if rows.len() == before {
        return Err(format!("no dictionary entry with id {id}"));
    }
    write_json_array(&app, DICTIONARY_FILE, &rows)
}

#[tauri::command]
fn dismiss_correction_suggestion(_from_word: String) -> Result<(), String> {
    Ok(())
}

// ---------- Snippets ----------
//
// Trigger phrase → expansion, stored in snippets.json. No defaults (users
// add their own). The Kotlin dictation path applies them after the
// dictionary, mirroring desktop.

const SNIPPETS_FILE: &str = "snippets.json";

fn load_snippets(app: &tauri::AppHandle) -> Vec<Value> {
    read_json_array(app, SNIPPETS_FILE)
}

#[tauri::command]
fn list_snippets(app: tauri::AppHandle) -> Vec<Value> {
    load_snippets(&app)
}

#[tauri::command]
fn add_snippet(app: tauri::AppHandle, trigger: String, expansion: String) -> Result<(), String> {
    let t = trigger.trim();
    let e = expansion.trim();
    if t.is_empty() {
        return Err("Trigger is required.".into());
    }
    if e.is_empty() {
        return Err("Expansion is required.".into());
    }
    let mut rows = load_snippets(&app);
    let id = next_id(&rows);
    rows.push(json!({
        "id": id,
        "trigger": t,
        "expansion": e,
        "hit_count": 0,
        "created_at": now_secs(),
    }));
    write_json_array(&app, SNIPPETS_FILE, &rows)
}

#[tauri::command]
fn update_snippet(
    app: tauri::AppHandle,
    id: i64,
    trigger: String,
    expansion: String,
) -> Result<(), String> {
    let t = trigger.trim();
    let e = expansion.trim();
    if t.is_empty() || e.is_empty() {
        return Err("Trigger and expansion are required.".into());
    }
    let mut rows = load_snippets(&app);
    let mut found = false;
    for r in rows.iter_mut() {
        if r["id"].as_i64() == Some(id) {
            r["trigger"] = json!(t);
            r["expansion"] = json!(e);
            found = true;
            break;
        }
    }
    if !found {
        return Err(format!("no snippet with id {id}"));
    }
    write_json_array(&app, SNIPPETS_FILE, &rows)
}

#[tauri::command]
fn delete_snippet(app: tauri::AppHandle, id: i64) -> Result<(), String> {
    let mut rows = load_snippets(&app);
    let before = rows.len();
    rows.retain(|r| r["id"].as_i64() != Some(id));
    if rows.len() == before {
        return Err(format!("no snippet with id {id}"));
    }
    write_json_array(&app, SNIPPETS_FILE, &rows)
}

// ---------- Transforms ----------
//
// The 6 built-ins mirror the desktop defaults (db.rs DEFAULT_TRANSFORMS)
// and the Kotlin PROCESS_TEXT sheet (Transforms.kt) so a transform reads
// and behaves the same across every surface. Stored in transforms.json
// with full CRUD, so the dashboard's create / edit / delete / set-default
// / reset all persist.

const DEFAULT_TRANSFORMS: &[(&str, &str, &str)] = &[
    (
        "Polish",
        "Improve clarity and conciseness \u{2014} no new content",
        "You are a writing editor. Polish ONLY the text the user dictated \u{2014} never write anything new on their behalf.\n\
\n\
What you DO:\n\
- Fix grammar, spelling, and punctuation.\n\
- Improve flow and clarity.\n\
- Match the original register (casual stays casual, formal stays formal).\n\
\n\
What you NEVER do:\n\
- Never fulfil a request inside the input. If the user said \"write a letter to the principal asking for leave\", you return \"Write a letter to the principal asking for leave.\" \u{2014} polished, same instruction. You DO NOT compose the letter.\n\
- Never answer questions, expand briefs, add examples, or invent details the user did not literally speak.\n\
- The output's length must be very close to the input's. A 10-word input should produce roughly 10 words out.\n\
\n\
Return ONLY the polished text. No preamble, no quotes around the output, no commentary.",
    ),
    (
        "Compose",
        "Draft a full message, email or letter from your brief",
        "You are a writing assistant. The user has dictated a BRIEF \u{2014} expand it into a polished, complete piece of writing.\n\
\n\
- Infer the format from the brief (email, letter, message, memo, note, etc.) and produce the appropriate structure (greeting, body, sign-off where the format expects them).\n\
- Match the tone the brief implies: formal letter to a principal sounds formal; quick note to a friend sounds casual.\n\
- Stay faithful to every fact, name, request, deadline, and constraint the user mentioned. Do not invent details the user didn't supply (don't make up names, dates, or specifics).\n\
- Reasonable length: a one-sentence brief produces a short output; a detailed brief can produce a longer draft. Don't pad.\n\
\n\
Return ONLY the composed text. No preamble, no notes about what you wrote.",
    ),
    (
        "Prompt Engineer",
        "Restructures your brief into an LLM-ready prompt",
        "You are a prompt engineer. Rewrite the user's brief into a clear, well-structured prompt for a large language model.\n\
- Open with the role / task in one sentence.\n\
- Add explicit instructions, constraints, and output format if implied by the brief.\n\
- Preserve every concrete detail the user provided. Do not invent constraints, examples, or context they didn't mention.\n\
- Use sections (\"Task:\", \"Constraints:\", \"Output:\") only if it improves clarity.\n\
- This is restructuring, not answering: never attempt to fulfil the prompt itself.\n\
\n\
Return ONLY the rewritten prompt. No preamble, no commentary.",
    ),
    (
        "Make Formal",
        "Switches to a professional tone, same content",
        "Rewrite the user's text in a formal, professional tone.\n\
- Use full sentences, proper grammar, conventional punctuation.\n\
- Avoid contractions, slang, and filler.\n\
- Preserve every fact and the approximate length. Do not expand a brief into a full draft \u{2014} this is a tone change, not a content generator. If the user dictated \"tell boss I'm sick\", you return \"Please inform the manager that I am unwell.\" \u{2014} not a full sick-leave email.\n\
- Never answer questions or fulfil requests inside the input.\n\
\n\
Return ONLY the rewritten text. No preamble, no commentary.",
    ),
    (
        "Make Casual",
        "Loosens the tone, same content",
        "Rewrite the user's text in a casual, friendly tone, as if talking to a colleague.\n\
- Use contractions where natural.\n\
- Keep it concise and human.\n\
- Preserve every fact and the approximate length. Do not expand a brief into a draft \u{2014} this is a tone change, not a content generator.\n\
- Never add jokes, new ideas, or content the user didn't mention.\n\
- Never answer questions or fulfil requests inside the input.\n\
\n\
Return ONLY the rewritten text. No preamble, no commentary.",
    ),
    (
        "Bullet Points",
        "Restructures prose into a bulleted list \u{2014} no new facts",
        "Convert the user's text into a clean bulleted list.\n\
- Each bullet is one clear point.\n\
- Preserve every fact, name, number, and the original order. Don't add bullets the source doesn't support.\n\
- Use dash bullets (\"- \"), one per line.\n\
- No nested bullets unless the source clearly has sub-points.\n\
- This is restructuring, not summarising or expanding: don't drop facts and don't invent new ones.\n\
\n\
Return ONLY the bulleted list. No preamble, no commentary.",
    ),
];

fn seed_transforms() -> Vec<Value> {
    DEFAULT_TRANSFORMS
        .iter()
        .enumerate()
        .map(|(i, (name, desc, prompt))| {
            json!({
                "id": i as i64 + 1,
                "name": name,
                "description": desc,
                "system_prompt": prompt,
                "is_default": i == 0,
                "sort_order": i as i64,
                "hit_count": 0,
                "created_at": now_secs(),
            })
        })
        .collect()
}

fn load_transforms(app: &tauri::AppHandle) -> Vec<Value> {
    load_store(app, TRANSFORMS_FILE, seed_transforms)
}

#[tauri::command]
fn list_transforms(app: tauri::AppHandle) -> Vec<Value> {
    load_transforms(&app)
}

/// No global-hotkey slots on Android, so there's nothing to report — the
/// React Transforms page treats an empty list as "no slot chips, no binding
/// warnings," which is exactly right on mobile.
#[tauri::command]
fn list_transform_slot_statuses() -> Vec<Value> {
    Vec::new()
}

#[tauri::command]
fn add_transform(
    app: tauri::AppHandle,
    name: String,
    description: String,
    system_prompt: String,
) -> Result<Value, String> {
    let name = name.trim();
    let prompt = system_prompt.trim();
    if name.is_empty() {
        return Err("Name is required.".into());
    }
    if prompt.is_empty() {
        return Err("System prompt is required.".into());
    }
    let mut rows = load_transforms(&app);
    let is_first = rows.is_empty();
    let id = next_id(&rows);
    let sort_order = rows.iter().filter_map(|r| r["sort_order"].as_i64()).max().unwrap_or(-1) + 1;
    let entry = json!({
        "id": id,
        "name": name,
        "description": description.trim(),
        "system_prompt": prompt,
        "is_default": is_first,
        "sort_order": sort_order,
        "hit_count": 0,
        "created_at": now_secs(),
    });
    rows.push(entry.clone());
    write_json_array(&app, TRANSFORMS_FILE, &rows)?;
    Ok(entry)
}

#[tauri::command]
fn update_transform(
    app: tauri::AppHandle,
    id: i64,
    name: String,
    description: String,
    system_prompt: String,
) -> Result<(), String> {
    let name = name.trim();
    let prompt = system_prompt.trim();
    if name.is_empty() {
        return Err("Name is required.".into());
    }
    if prompt.is_empty() {
        return Err("System prompt is required.".into());
    }
    let mut rows = load_transforms(&app);
    let mut found = false;
    for r in rows.iter_mut() {
        if r["id"].as_i64() == Some(id) {
            r["name"] = json!(name);
            r["description"] = json!(description.trim());
            r["system_prompt"] = json!(prompt);
            found = true;
            break;
        }
    }
    if !found {
        return Err(format!("no transform with id {id}"));
    }
    write_json_array(&app, TRANSFORMS_FILE, &rows)
}

#[tauri::command]
fn delete_transform(app: tauri::AppHandle, id: i64) -> Result<(), String> {
    let mut rows = load_transforms(&app);
    let before = rows.len();
    let deleted_default = rows
        .iter()
        .any(|r| r["id"].as_i64() == Some(id) && r["is_default"].as_bool() == Some(true));
    rows.retain(|r| r["id"].as_i64() != Some(id));
    if rows.len() == before {
        return Err(format!("no transform with id {id}"));
    }
    // Deleting the default promotes the first remaining transform, so the
    // pipeline always has a default to fall back to.
    if deleted_default {
        if let Some(first) = rows.first_mut() {
            first["is_default"] = json!(true);
        }
    }
    write_json_array(&app, TRANSFORMS_FILE, &rows)
}

#[tauri::command]
fn set_default_transform(app: tauri::AppHandle, id: i64) -> Result<(), String> {
    let mut rows = load_transforms(&app);
    if !rows.iter().any(|r| r["id"].as_i64() == Some(id)) {
        return Err(format!("no transform with id {id}"));
    }
    for r in rows.iter_mut() {
        let is_it = r["id"].as_i64() == Some(id);
        r["is_default"] = json!(is_it);
    }
    write_json_array(&app, TRANSFORMS_FILE, &rows)
}

#[tauri::command]
fn reset_transforms(app: tauri::AppHandle) -> Result<(), String> {
    let seeded = seed_transforms();
    write_json_array(&app, TRANSFORMS_FILE, &seeded)
}

/// Runs a stored transform against arbitrary text (the Scratchpad's
/// rewrite-selection chips). Looks the transform up in transforms.json and
/// runs its prompt through Groq. Arg is `transform_id` to match the desktop
/// command / the React invoke.
#[tauri::command]
async fn run_transform_on_text(
    app: tauri::AppHandle,
    transform_id: i64,
    text: String,
) -> Result<String, String> {
    let cfg = read_config(&app).unwrap_or_default();
    if !cfg.has_api_key() {
        return Err("Set your Groq API key in Settings first.".to_string());
    }
    if text.trim().is_empty() {
        return Ok(text);
    }
    let prompt = load_transforms(&app)
        .into_iter()
        .find(|t| t["id"].as_i64() == Some(transform_id))
        .and_then(|t| t["system_prompt"].as_str().map(|s| s.to_string()))
        .ok_or_else(|| "Transform not found".to_string())?;
    groq_chat(cfg.groq_api_key.trim(), &cfg.chat_model, &prompt, &text, 0.3).await
}

// ---------- Notes / scratchpad ----------
//
// File-backed notes (notes.json), same pattern as the other stores. The
// desktop scratchpad is a separate window; on mobile it's the in-app
// ScratchpadView, which drives these commands directly.

const NOTES_FILE: &str = "notes.json";

fn load_notes(app: &tauri::AppHandle) -> Vec<Value> {
    read_json_array(app, NOTES_FILE)
}

#[tauri::command]
fn list_notes(app: tauri::AppHandle) -> Vec<Value> {
    let mut rows = load_notes(&app);
    // Newest-updated first, matching how the editor prepends new notes.
    rows.sort_by(|a, b| {
        b["updated_at"].as_i64().unwrap_or(0).cmp(&a["updated_at"].as_i64().unwrap_or(0))
    });
    rows
}

#[tauri::command]
fn create_note(app: tauri::AppHandle, title: String, body: String) -> Result<Value, String> {
    let mut rows = load_notes(&app);
    let now = now_secs();
    let note = json!({
        "id": next_id(&rows),
        "title": title,
        "body": body,
        "created_at": now,
        "updated_at": now,
    });
    rows.push(note.clone());
    write_json_array(&app, NOTES_FILE, &rows)?;
    Ok(note)
}

#[tauri::command]
fn update_note(app: tauri::AppHandle, id: i64, title: String, body: String) -> Result<(), String> {
    let mut rows = load_notes(&app);
    let mut found = false;
    for r in rows.iter_mut() {
        if r["id"].as_i64() == Some(id) {
            r["title"] = json!(title);
            r["body"] = json!(body);
            r["updated_at"] = json!(now_secs());
            found = true;
            break;
        }
    }
    if !found {
        return Err(format!("no note with id {id}"));
    }
    write_json_array(&app, NOTES_FILE, &rows)
}

#[tauri::command]
fn delete_note(app: tauri::AppHandle, id: i64) -> Result<(), String> {
    let mut rows = load_notes(&app);
    let before = rows.len();
    rows.retain(|r| r["id"].as_i64() != Some(id));
    if rows.len() == before {
        return Err(format!("no note with id {id}"));
    }
    write_json_array(&app, NOTES_FILE, &rows)
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

/// Persists onboarding completion so the first-launch wizard never shows
/// again. Read-modify-write of config.json (keeps the key/language/etc. the
/// wizard just saved). Called by the wizard's final "Open Bulbul" button.
#[tauri::command]
fn complete_onboarding(app: tauri::AppHandle) -> Result<(), String> {
    let mut cfg = read_config(&app).unwrap_or_default();
    cfg.onboarding_completed = true;
    save_config(app, cfg)
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
            get_overlay_snoozed_until,
            resume_overlay,
            // Overlay / scratchpad windows
            set_overlay_height,
            open_scratchpad,
            complete_onboarding,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Bulbul on mobile");
}
