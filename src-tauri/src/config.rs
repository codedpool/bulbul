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
                "Remove filler words (um, uh, like, you know). Fix punctuation and capitalization. Preserve meaning exactly. Do not paraphrase.\n\n\
                 Bullet-list detection: if the speaker is clearly enumerating distinct items (signalled by 'first ... second ... third ...', 'one ... two ... three ...', 'and another thing ...', 'also ...', or a bare list of nouns), format the items as a markdown bullet list, one item per line, prefixed with '- '. Drop the enumerator words themselves. Do NOT bulletize ordinary prose, single-sentence answers, or examples woven into a sentence."
            }
            CleanupMode::Polished => {
                "Rewrite into clean, natural prose. Remove self-corrections and filler. Fix flow. Keep the speaker's original intent and key facts. Return only the rewritten text.\n\n\
                 Bullet-list detection: if the speaker is clearly enumerating distinct items (signalled by 'first ... second ... third ...', 'one ... two ... three ...', 'and another thing ...', 'also ...', or a bare list of nouns), format the items as a markdown bullet list, one item per line, prefixed with '- '. Drop the enumerator words themselves. Do NOT bulletize ordinary prose, single-sentence answers, or examples woven into a sentence."
            }
        }
    }
}

/// User-defined mapping from a foreground executable to a Style category.
/// Overrides Bulbul's built-in `style_category_for_app` defaults, so a user
/// can route e.g. "Cursor.exe" to "work" instead of "other".
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppOverride {
    /// Executable name, with or without the .exe suffix. Matched
    /// case-insensitively against the foreground process's image name.
    pub exe: String,
    /// One of "personal" | "work" | "email" | "other".
    pub category: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub groq_api_key: String,

    #[serde(default)]
    pub mode: CleanupMode,

    #[serde(default = "default_hotkey")]
    pub hotkey: String,

    /// Push-to-talk hotkey that records audio and runs it through the
    /// pipeline with CleanupMode::Polished forced — the user gets a
    /// rewritten-for-clarity output regardless of their global cleanup
    /// mode. Single LLM call (cleanup), same latency as normal dictation.
    /// Alias covers users upgrading from default_transform_hotkey.
    #[serde(
        default = "default_polish_hotkey",
        alias = "default_transform_hotkey",
        alias = "voice_transform_hotkey"
    )]
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

    /// User-defined exe → category overrides. Empty by default; users add
    /// rows from the Style page when the built-in mappings don't cover an
    /// app they care about (e.g. routing "Cursor.exe" to work).
    #[serde(default)]
    pub style_app_overrides: Vec<AppOverride>,

    #[serde(default = "default_personalize_cleanup")]
    pub personalize_cleanup: bool,

    #[serde(default = "default_learn_corrections")]
    pub learn_corrections: bool,

    /// UI theme preference: "light" (default) | "dark" | "system".
    #[serde(default = "default_theme")]
    pub theme: String,

    /// True once the user has finished (or explicitly skipped) the
    /// first-run wizard. Defaults to false so fresh installs see it.
    #[serde(default)]
    pub onboarding_completed: bool,

    /// Anonymous usage telemetry. On by default for fresh installs so the
    /// solo-dev signal isn't permanently zero, but always toggleable from
    /// onboarding (the toggle is visible on the final step) and from
    /// Settings → Privacy. Content (transcripts, audio, foreground exe
    /// names) is never sent regardless of this flag; only counts,
    /// durations, modes, and error categories. See [crate::telemetry] for
    /// the full taxonomy.
    #[serde(default = "default_telemetry_enabled")]
    pub telemetry_enabled: bool,
}

fn default_polish_hotkey() -> String {
    // Shift+Alt+P. Chosen to NOT share modifiers with the typical
    // hold-to-talk dictation chord (which uses Win) — so the
    // modifier-chord watcher and this RegisterHotKey combo never race.
    // Also avoids Ctrl+Shift+P, which is the VSCode command palette.
    "Shift+Alt+P".to_string()
}

fn default_hotkey() -> String {
    // Modifier-only chord, modifier-only hold-to-talk. The keyboard
    // hook (see hotkey.rs::spawn_modifier_chord_watcher) detects this as
    // a dictation press once both modifiers have been held for ~80ms.
    // Existing users with a previously-saved hotkey keep theirs; this
    // default only applies to fresh installs.
    "Ctrl+Win".to_string()
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
fn default_personalize_cleanup() -> bool { false }
fn default_learn_corrections() -> bool { true }
fn default_theme() -> String { "light".to_string() }
fn default_telemetry_enabled() -> bool { true }
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

/// Map an executable name (e.g. "Code.exe") to a human-readable app name
/// the LLM is likely to recognize from its training data. This becomes the
/// venue hint we append to the cleanup system prompt so the model can adapt
/// formatting conventions (markdown in Slack vs. literal punctuation in a
/// shell vs. paragraphs in Outlook).
///
/// Returns the bare stem (e.g. "Foo" for "Foo.exe") when the exe isn't in
/// the curated table — better to surface *something* and let the model use
/// its world knowledge than to silently drop the signal.
pub fn friendly_app_name(exe: &str) -> String {
    let lower = exe.to_lowercase();
    let stem = lower.trim_end_matches(".exe");
    let mapped = match stem {
        // Editors / IDEs
        "code" => "VS Code",
        "cursor" => "Cursor",
        "windsurf" => "Windsurf",
        "devenv" => "Visual Studio",
        "idea64" | "idea" => "IntelliJ IDEA",
        "pycharm64" | "pycharm" => "PyCharm",
        "webstorm64" | "webstorm" => "WebStorm",
        "sublime_text" => "Sublime Text",
        // Shells / terminals
        "windowsterminal" => "Windows Terminal",
        "pwsh" => "PowerShell",
        "powershell" => "Windows PowerShell",
        "cmd" => "Command Prompt",
        "wezterm-gui" | "wezterm" => "WezTerm",
        "alacritty" => "Alacritty",
        // Chat / collab
        "slack" => "Slack",
        "teams" | "ms-teams" => "Microsoft Teams",
        "discord" => "Discord",
        "whatsapp" => "WhatsApp",
        "telegram" => "Telegram",
        "signal" => "Signal",
        "messenger" => "Messenger",
        "zoom" => "Zoom",
        // Email
        "outlook" => "Outlook",
        "thunderbird" => "Thunderbird",
        "hostedgmaildesktopapp" => "Gmail",
        // Browsers (weak signal — let the model decide)
        "chrome" => "Google Chrome",
        "msedge" => "Microsoft Edge",
        "firefox" => "Firefox",
        "brave" => "Brave",
        "arc" => "Arc",
        // Notes / docs
        "notion" => "Notion",
        "obsidian" => "Obsidian",
        "evernote" => "Evernote",
        "winword" => "Microsoft Word",
        "excel" => "Microsoft Excel",
        "powerpnt" => "Microsoft PowerPoint",
        "onenote" => "OneNote",
        "notepad" => "Notepad",
        // Other
        "linear" => "Linear",
        "figma" => "Figma",
        _ => "",
    };
    if !mapped.is_empty() {
        return mapped.to_string();
    }
    // Fallback: strip the trailing .exe (case-insensitively) from the
    // original input so we keep the user's case (e.g. "MyApp" not "myapp").
    if let Some(idx) = exe.to_lowercase().rfind(".exe") {
        if idx == exe.len() - 4 {
            return exe[..idx].to_string();
        }
    }
    exe.to_string()
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
            style_app_overrides: Vec::new(),
            personalize_cleanup: default_personalize_cleanup(),
            learn_corrections: default_learn_corrections(),
            theme: default_theme(),
            onboarding_completed: false,
            telemetry_enabled: default_telemetry_enabled(),
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

    /// Resolve a foreground executable to a Style category, consulting the
    /// user's overrides first and falling back to the built-in mapping.
    /// Matching is case-insensitive on the stem (so "code.exe", "Code.exe"
    /// and "Code" all hit the same override).
    pub fn category_for_app(&self, exe: Option<&str>) -> &'static str {
        if let Some(name) = exe {
            let stem = name.to_lowercase();
            let stem = stem.trim_end_matches(".exe");
            for ov in &self.style_app_overrides {
                let ov_stem = ov.exe.to_lowercase();
                let ov_stem = ov_stem.trim_end_matches(".exe");
                if !ov_stem.is_empty() && ov_stem == stem {
                    return match ov.category.as_str() {
                        "personal" => "personal",
                        "work" => "work",
                        "email" => "email",
                        _ => "other",
                    };
                }
            }
        }
        style_category_for_app(exe)
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
