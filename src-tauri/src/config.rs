use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

// Self-correction handling is shared between Clean and Polished modes.
// When the speaker revises themselves mid-utterance the user almost
// always wants the FINAL value kept and the cancelled value dropped —
// that's the whole point of dictating "no no 5pm" instead of editing
// the text after the fact. Without an explicit rule the model
// faithfully transcribes "schedule a meeting for 4pm, no no, 5pm",
// which is exactly the wrong output for a chat / calendar / notes
// venue. We give the model both a trigger list (the verbal cues that
// signal a self-correction) and two concrete examples; 8B models
// follow few-shot patterns far more reliably than abstract rules.
//
// Raw mode deliberately does NOT do this — Raw promises every word and
// every disfluency, which is what a transcription-archive user wants.
macro_rules! self_correction_rule {
    () => {
        "Resolve self-corrections to the speaker's final choice. When the speaker revises themselves mid-utterance (signalled by \"no no\", \"no wait\", \"wait\", \"actually\", \"I mean\", \"scratch that\", \"or rather\", \"make it\", \"sorry\"), keep ONLY the value the speaker landed on. Drop the cancelled value AND the correction marker — neither should appear in the output.\n\n\
         Examples (resolve self-correction → final intent):\n\
         Spoken:  \"schedule a meeting for 4 PM no no 5 PM\"\n\
         Output:  \"Schedule a meeting for 5 PM.\"\n\n\
         Spoken:  \"send him 50 dollars wait make it 100\"\n\
         Output:  \"Send him 100 dollars.\"\n\n\
         Spoken:  \"the deadline is Friday actually Monday\"\n\
         Output:  \"The deadline is Monday.\"\n\n\
         Caveat — these signal words are real English too. Drop them ONLY when they introduce a revision of the immediately preceding noun phrase, time, number, name, or short value. A sentence like \"we actually shipped it last week\" is not a self-correction; \"actually\" is just an intensifier and must stay."
    };
}

// The bullet-list rule is shared between Clean and Polished modes. It's
// a macro (not a const) because `system_instruction` returns
// `&'static str` built via `concat!`, which only accepts literals.
//
// Why so explicit: the old wording ("format the items as a bullet list,
// drop the enumerator words, do not bulletize ordinary prose") produced
// hybrid output — small models would keep the surrounding sentence AND
// add the bullets, which is exactly the bug we're trying to fix. The
// new wording locks down three things:
//
//   1. Strict entry criteria — only convert when ALL three conditions
//      hold. Drops loose triggers like "also" and "another thing" that
//      caused false bulletisation of normal conversational sentences.
//
//   2. Full-replacement output — the entire response IS the bullet
//      list. The model used to add a lead-in ("Here are the items:")
//      or a trailing summary; both are explicitly forbidden now.
//
//   3. A concrete before/after example — `llama-3.1-8b-instant`
//      anchors on few-shot examples much more reliably than on
//      abstract rules.
macro_rules! bullet_rule {
    () => {
        "Bullet-list rule (strict). Convert to a bullet list ONLY when ALL of the following are true:\n\
         (a) the speaker is enumerating at least 2 distinct items;\n\
         (b) the enumeration is signalled by an EXPLICIT cue — ordinal markers (\"first... second... third...\", \"one... two... three...\"), an explicit lead-in (\"here are the things...\", \"the items are...\", \"I need...:\"), or a bare list of nouns joined by commas/and (\"milk, eggs, bread, and coffee\");\n\
         (c) each item stands on its own as a short list entry.\n\n\
         When converting, the ENTIRE OUTPUT is the bullet list, one item per line, each prefixed with \"- \". NO introductory sentence, NO trailing sentence, NO lead-in like \"Here are the items:\". Drop the enumerator words (\"first\", \"second\", \"one\", \"two\", etc.) from each bullet's text.\n\n\
         Example:\n\
         Spoken: \"first I need to buy milk, second eggs, and third some bread\"\n\
         Output:\n\
         - milk\n\
         - eggs\n\
         - bread\n\n\
         Do NOT bulletise: normal sentences containing \"also\" / \"another thing\" / \"by the way\"; examples woven into prose; single-sentence answers; explanations with a few enumerated points (those stay as prose with the ordinal words intact). When in doubt, keep prose."
    };
}

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
                "Fix only obvious transcription errors. Keep every word and every disfluency, including self-corrections — the user has explicitly chosen the raw mode to see what they actually said."
            }
            CleanupMode::Clean => {
                concat!(
                    "Remove filler words (um, uh, like, you know). Fix punctuation and capitalization. Beyond fillers and self-corrections (rule below), preserve every word and the speaker's meaning. Do not paraphrase.\n\n",
                    self_correction_rule!(),
                    "\n\n",
                    bullet_rule!(),
                )
            }
            CleanupMode::Polished => {
                concat!(
                    "Rewrite into clean, natural prose. Remove filler and tighten flow. Keep the speaker's original intent and key facts. Return only the rewritten text.\n\n",
                    self_correction_rule!(),
                    "\n\n",
                    bullet_rule!(),
                )
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

    /// User's preferred first name / display name. Optional. Stays
    /// local — never sent to any backend. Used for:
    ///   - "Welcome back, X" greeting on the Home page
    ///   - Sign-offs in Compose-style transforms (replaces the
    ///     [Your Name] placeholder the model would otherwise emit)
    ///   - Personal touch in other surfaces over time
    /// Empty string means "no name set", in which case sign-offs omit
    /// the name line entirely rather than using a placeholder.
    #[serde(default = "default_display_name")]
    pub display_name: String,

    /// When true, the system-tray icon is hidden. The app keeps running
    /// in the background and the hotkey still works; the dashboard is
    /// reached by re-launching Bulbul (single-instance focuses the
    /// existing window). An in-dashboard Quit button appears in this
    /// mode so the user isn't stranded without a way to exit.
    #[serde(default = "default_hide_tray")]
    pub hide_tray: bool,

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

    /// Floating overlay bubble appearance (Android only). `overlay_opacity`
    /// is 0.3–1.0, `overlay_size` is the bubble diameter in dp. Desktop
    /// ignores these — its pill is sized by the window/CSS — but they live
    /// in the shared Config so the mobile Settings page (React) and the
    /// Kotlin foreground service agree on where the values are stored.
    #[serde(default = "default_overlay_opacity")]
    pub overlay_opacity: f32,
    #[serde(default = "default_overlay_size")]
    pub overlay_size: u32,
    /// How long the overlay stays snoozed when dropped on the snooze target,
    /// in minutes (Android only). Default 1 hour.
    #[serde(default = "default_overlay_snooze_minutes")]
    pub overlay_snooze_minutes: u32,
}

fn default_polish_hotkey() -> String {
    // Shift+Alt+P. Chosen to NOT share modifiers with the typical
    // hold-to-talk dictation chord (which uses Win) — so the
    // modifier-chord watcher and this RegisterHotKey combo never race.
    // Also avoids Ctrl+Shift+P, which is the VSCode command palette.
    "Shift+Alt+P".to_string()
}

fn default_hotkey() -> String {
    // Modifier-only chord, hold-to-talk. The keyboard
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
fn default_display_name() -> String {
    String::new()
}
fn default_hide_tray() -> bool {
    false
}
fn default_language() -> String {
    "auto".to_string()
}
fn default_style_enabled() -> bool { true }
fn default_personalize_cleanup() -> bool { false }
fn default_learn_corrections() -> bool { true }
fn default_theme() -> String { "light".to_string() }
fn default_telemetry_enabled() -> bool { true }
fn default_overlay_opacity() -> f32 { 0.65 }
fn default_overlay_size() -> u32 { 52 }
fn default_overlay_snooze_minutes() -> u32 { 60 }
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
        // Editors / IDEs — Windows exe stems + Linux WM_CLASS (most overlap)
        "code" => "VS Code",
        "cursor" => "Cursor",
        "windsurf" => "Windsurf",
        "devenv" => "Visual Studio",
        "idea64" | "idea" | "jetbrains-idea" => "IntelliJ IDEA",
        "pycharm64" | "pycharm" | "jetbrains-pycharm" => "PyCharm",
        "webstorm64" | "webstorm" | "jetbrains-webstorm" => "WebStorm",
        "sublime_text" => "Sublime Text",
        "gedit" | "org.gnome.gedit" | "gnome-text-editor" => "GNOME Text Editor",
        "kate" | "org.kde.kate" => "Kate",
        // Shells / terminals
        "windowsterminal" => "Windows Terminal",
        "pwsh" => "PowerShell",
        "powershell" => "Windows PowerShell",
        "cmd" => "Command Prompt",
        "wezterm-gui" | "wezterm" => "WezTerm",
        "alacritty" => "Alacritty",
        "org.gnome.terminal" | "gnome-terminal" | "gnome-terminal-server" => "GNOME Terminal",
        "org.kde.konsole" | "konsole" => "Konsole",
        "xterm" => "XTerm",
        // Chat / collab
        "slack" => "Slack",
        "teams" | "ms-teams" => "Microsoft Teams",
        "discord" => "Discord",
        "whatsapp" => "WhatsApp",
        "telegram" | "telegramdesktop" => "Telegram",
        "signal" | "signal-desktop" => "Signal",
        "messenger" => "Messenger",
        "zoom" => "Zoom",
        // Email
        "outlook" => "Outlook",
        "thunderbird" | "mozilla thunderbird" => "Thunderbird",
        "hostedgmaildesktopapp" => "Gmail",
        "evolution" => "Evolution",
        // Browsers (weak signal — let the model decide)
        "chrome" | "google-chrome" => "Google Chrome",
        "msedge" => "Microsoft Edge",
        "firefox" | "navigator" => "Firefox",
        "brave" | "brave-browser" => "Brave",
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
        "libreoffice" | "libreoffice-writer" | "libreoffice-calc" => "LibreOffice",
        // Other
        "linear" => "Linear",
        "figma" => "Figma",
        "spotify" => "Spotify",

        // --- macOS bundle IDs ---
        // Apple
        "com.apple.safari" => "Safari",
        "com.apple.terminal" => "Terminal",
        "com.apple.mail" => "Mail",
        "com.apple.messages" => "Messages",
        "com.apple.finder" => "Finder",
        "com.apple.notes" => "Notes",
        "com.apple.textedit" => "TextEdit",
        "com.apple.dt.xcode" => "Xcode",
        "com.apple.iwork.pages" => "Pages",
        "com.apple.iwork.numbers" => "Numbers",
        "com.apple.iwork.keynote" => "Keynote",
        // Microsoft on Mac
        "com.microsoft.vscode" => "VS Code",
        "com.microsoft.word" => "Microsoft Word",
        "com.microsoft.excel" => "Microsoft Excel",
        "com.microsoft.powerpoint" => "Microsoft PowerPoint",
        "com.microsoft.outlook" => "Outlook",
        "com.microsoft.teams" | "com.microsoft.teams2" => "Microsoft Teams",
        "com.microsoft.edgemac" => "Microsoft Edge",
        // Chat / collab
        "com.tinyspeck.slackmacgap" => "Slack",
        "com.hnc.discord" => "Discord",
        "ru.keepcoder.telegram" | "org.telegram.desktop" => "Telegram",
        "net.whatsapp.whatsapp" => "WhatsApp",
        "org.whispersystems.signal-desktop" => "Signal",
        "us.zoom.xos" => "Zoom",
        // Browsers
        "com.brave.browser" => "Brave",
        "com.google.chrome" => "Google Chrome",
        "org.mozilla.firefox" => "Firefox",
        "company.thebrowser.browser" => "Arc",
        // Notes / docs
        "notion.id" | "com.notion.id" => "Notion",
        "md.obsidian" => "Obsidian",
        "com.evernote.evernote" => "Evernote",
        // Dev tools / editors
        "com.todesktop.230313mzl4w4u92" => "Cursor",
        "com.exafunction.windsurf" => "Windsurf",
        "com.jetbrains.intellij" => "IntelliJ IDEA",
        "com.jetbrains.pycharm" => "PyCharm",
        "com.sublimetext.4" | "com.sublimetext.3" => "Sublime Text",
        // Productivity
        "com.figma.desktop" => "Figma",
        "com.linear-app.linear" => "Linear",
        "com.spotify.client" => "Spotify",

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
        // Personal chat
        "whatsapp" | "telegram" | "telegramdesktop" | "signal" | "signal-desktop"
        | "messenger"
        | "com.apple.messages"
        | "net.whatsapp.whatsapp"
        | "ru.keepcoder.telegram" | "org.telegram.desktop"
        | "org.whispersystems.signal-desktop" => "personal",
        // Work chat / collab
        "slack" | "teams" | "ms-teams" | "discord"
        | "com.tinyspeck.slackmacgap"
        | "com.microsoft.teams" | "com.microsoft.teams2"
        | "com.hnc.discord" => "work",
        // Email
        "outlook" | "thunderbird" | "mozilla thunderbird" | "hostedgmaildesktopapp"
        | "evolution"
        | "com.apple.mail"
        | "com.microsoft.outlook" => "email",
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
            display_name: default_display_name(),
            hide_tray: default_hide_tray(),
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
            overlay_opacity: default_overlay_opacity(),
            overlay_size: default_overlay_size(),
            overlay_snooze_minutes: default_overlay_snooze_minutes(),
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
