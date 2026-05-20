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
}

fn default_hotkey() -> String {
    "Ctrl+Shift+Space".to_string()
}
fn default_polish_hotkey() -> String {
    "Ctrl+Shift+P".to_string()
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
