use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum CleanupMode {
    #[serde(rename = "raw")]
    Raw,
    #[serde(rename = "clean")]
    Clean,
    #[serde(rename = "polished")]
    Polished,
}

impl Default for CleanupMode {
    fn default() -> Self {
        CleanupMode::Clean
    }
}

impl CleanupMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            CleanupMode::Raw => "raw",
            CleanupMode::Clean => "clean",
            CleanupMode::Polished => "polished",
        }
    }

    pub fn system_instruction(&self) -> &'static str {
        match self {
            CleanupMode::Raw => {
                "Fix only obvious transcription errors. Keep every word and every disfluency."
            }
            CleanupMode::Clean => {
                "Remove filler words (um, uh, like, you know). Fix punctuation and capitalization. Preserve meaning exactly. Do not paraphrase."
            }
            CleanupMode::Polished => {
                "Rewrite into clean, natural prose. Remove self-corrections and filler. Fix flow. Keep the speaker's original intent and key facts. Return only the rewritten text."
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub groq_api_key: String,

    #[serde(default)]
    pub mode: CleanupMode,

    #[serde(default = "default_hotkey")]
    pub hotkey: String,

    #[serde(default = "default_polish_hotkey")]
    pub polish_hotkey: String,

    #[serde(default = "default_stt_model")]
    pub stt_model: String,

    #[serde(default = "default_chat_model")]
    pub chat_model: String,

    #[serde(default = "default_min_seconds")]
    pub min_recording_seconds: f32,

    #[serde(default = "default_privacy_ack")]
    pub privacy_acknowledged: bool,

    #[serde(default = "default_open_dashboard")]
    pub open_dashboard_on_launch: bool,

    #[serde(default = "default_language")]
    pub language: String,

    #[serde(default = "default_style_enabled")]
    pub style_enabled: bool,
    #[serde(default = "default_style_personal")]
    pub style_personal: String,
    #[serde(default = "default_style_work")]
    pub style_work: String,
    #[serde(default = "default_style_email")]
    pub style_email: String,
    #[serde(default = "default_style_other")]
    pub style_other: String,

    #[serde(default = "default_personalize_cleanup")]
    pub personalize_cleanup: bool,
}

fn default_hotkey() -> String {
    "Ctrl+Shift+Space".to_string()
}
fn default_polish_hotkey() -> String {
    // Win+Alt+P matches the commercial dictation apps convention. Bare Alt combos
    // briefly activate the Windows menu bar and deselect text before
    // our Ctrl+C runs, so we always include Win as the lead modifier
    // for transform-style hotkeys. Existing users keep whatever they had.
    "Win+Alt+P".to_string()
}
fn default_stt_model() -> String {
    "whisper-large-v3-turbo".to_string()
}
fn default_chat_model() -> String {
    "llama-3.1-8b-instant".to_string()
}
fn default_min_seconds() -> f32 {
    0.4
}
fn default_privacy_ack() -> bool {
    false
}
fn default_open_dashboard() -> bool {
    true
}
fn default_language() -> String {
    "auto".to_string()
}
fn default_style_enabled() -> bool { true }
fn default_personalize_cleanup() -> bool { true }
fn default_style_personal() -> String { "casual".to_string() }
fn default_style_work() -> String { "casual".to_string() }
fn default_style_email() -> String { "formal".to_string() }
fn default_style_other() -> String { "casual".to_string() }

/// Map a Style preset key to a short instruction appended to the cleanup
/// system prompt. Returns None for "raw" or any unrecognized value.
pub fn style_modifier(style: &str) -> Option<&'static str> {
    match style {
        "formal" => Some(
            "Style: formal. Use proper capitalization and full punctuation. Use complete sentences, avoid contractions and slang.",
        ),
        "casual" => Some(
            "Style: casual. Use natural capitalization and standard punctuation. Conversational tone, contractions allowed.",
        ),
        "very_casual" => Some(
            "Style: very casual. Skip sentence-start capitalization where natural. Minimize punctuation (no full stops, fewer commas). Keep it brief and informal — like a quick text.",
        ),
        _ => None,
    }
}

/// Map an executable name (e.g. "WhatsApp.exe") to a Style category.
/// Mirrors the Insights categorization but coarser — we only care about
/// personal / work / email / other for Style.
pub fn style_category_for_app(exe: Option<&str>) -> &'static str {
    let Some(exe) = exe else { return "other" };
    let lower = exe.to_lowercase();
    let stem = lower.trim_end_matches(".exe");
    match stem {
        "whatsapp" | "telegram" | "signal" | "messenger" => "personal",
        "slack" | "teams" | "discord" => "work",
        "outlook" | "thunderbird" | "hostedgmaildesktopapp" => "email",
        _ => "other",
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            groq_api_key: String::new(),
            mode: CleanupMode::default(),
            hotkey: default_hotkey(),
            polish_hotkey: default_polish_hotkey(),
            stt_model: default_stt_model(),
            chat_model: default_chat_model(),
            min_recording_seconds: default_min_seconds(),
            privacy_acknowledged: default_privacy_ack(),
            open_dashboard_on_launch: default_open_dashboard(),
            language: default_language(),
            style_enabled: default_style_enabled(),
            style_personal: default_style_personal(),
            style_work: default_style_work(),
            style_email: default_style_email(),
            style_other: default_style_other(),
            personalize_cleanup: default_personalize_cleanup(),
        }
    }
}

impl Config {
    pub fn style_for_category(&self, category: &str) -> &str {
        match category {
            "personal" => &self.style_personal,
            "work" => &self.style_work,
            "email" => &self.style_email,
            _ => &self.style_other,
        }
    }
}

impl Config {
    pub fn has_api_key(&self) -> bool {
        !self.groq_api_key.trim().is_empty()
    }
}



pub fn config_dir() -> Result<PathBuf> {
    let base = dirs::config_dir().context("could not resolve %APPDATA%")?;
    let dir = base.join("Bulbul");
    fs::create_dir_all(&dir).with_context(|| format!("creating {:?}", dir))?;
    Ok(dir)
}

pub fn config_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("config.json"))
}

pub fn load() -> Config {
    let path = match config_path() {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("could not resolve config path: {e:#}");
            return Config::default();
        }
    };
    match fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str::<Config>(&text) {
            Ok(cfg) => cfg,
            Err(e) => {
                tracing::warn!("invalid config at {path:?}: {e:#} — using defaults");
                Config::default()
            }
        },
        Err(_) => Config::default(),
    }
}

pub fn save(cfg: &Config) -> Result<()> {
    let path = config_path()?;
    let text = serde_json::to_string_pretty(cfg)?;
    fs::write(&path, text).with_context(|| format!("writing {path:?}"))?;
    Ok(())
}
