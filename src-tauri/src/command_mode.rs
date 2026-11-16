// SPDX-License-Identifier: GPL-3.0-only
// Copyright (c) 2026 Romanch Roshan Singh

//! Command mode (desktop): hold the command hotkey, speak an instruction,
//! release.
//!
//! - **Text selected:** the instruction is applied to it and the result
//!   replaces the selection ("make this friendlier", "translate to Hindi",
//!   "write an essay about dogs here").
//! - **Nothing selected:** the instruction is carried out at the cursor.
//!
//! The recording itself is handled by the orchestrator, like dictation. This
//! is what runs after release: speech-to-text for the instruction and the
//! grab of the selection happen together (the user has let go of the hotkey's
//! modifier keys by then, so a synthesised Ctrl+C is a plain copy), then the
//! model, then the paste. The selection is only replaced once a result has
//! arrived: any failure before that leaves the user's text untouched.

use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager};

use crate::config::Config;
use crate::hotkey::{MouseButton, ParsedHotkey};
use crate::{db, groq, inject, stt, AppState};

/// The hotkey to register for command mode: the configured combo while the
/// feature is on, nothing (no key, so nothing registers) while it's off.
pub fn hotkey(cfg: &Config) -> ParsedHotkey {
    if cfg.command_mode_enabled {
        ParsedHotkey::parse(&cfg.command_hotkey)
    } else {
        ParsedHotkey::default()
    }
}

/// The mouse button bound to command mode, if the feature is on and one is set.
pub fn mouse_button(cfg: &Config) -> Option<MouseButton> {
    if !cfg.command_mode_enabled {
        return None;
    }
    match cfg.command_mouse_button.trim() {
        "" | "none" => None,
        other => Some(MouseButton::parse(other)),
    }
}

/// Programs where a synthesised Ctrl+C is not "copy": in a terminal it
/// interrupts whatever is running. Selection is never grabbed with Ctrl+C
/// there (on Linux the PRIMARY selection still works).
pub fn is_terminal(app_id: Option<&str>) -> bool {
    let Some(id) = app_id else { return false };
    let lower = id.to_lowercase();
    let stem = lower.trim_end_matches(".exe");
    matches!(
        stem,
        "windowsterminal"
            | "wt"
            | "cmd"
            | "powershell"
            | "pwsh"
            | "conhost"
            | "openconsole"
            | "mintty"
            | "wezterm-gui"
            | "wezterm"
            | "alacritty"
            | "kitty"
            | "foot"
            | "xterm"
            | "urxvt"
            | "rxvt"
            | "st"
            | "terminator"
            | "tilix"
            | "konsole"
            | "gnome-terminal"
            | "gnome-terminal-server"
            | "xfce4-terminal"
            | "mate-terminal"
            | "lxterminal"
            | "hyper"
            | "ghostty"
            | "warp"
            | "org.gnome.terminal"
            | "org.kde.konsole"
            | "io.alacritty"
            | "net.kovidgoyal.kitty"
            | "com.mitchellh.ghostty"
            | "dev.warp.warp"
            | "com.apple.terminal"
            | "com.googlecode.iterm2"
            | "iterm2"
    )
}

/// What the selection grab found.
struct Selection {
    /// The selected text, when there was some.
    text: Option<String>,
    /// The clipboard's previous contents, set only when the grab replaced them
    /// (the Ctrl+C route), so they can be put back.
    original: Option<String>,
}

/// Reads the highlighted text. Linux reads the PRIMARY selection (nothing is
/// copied, the clipboard is untouched); everywhere else, and on Linux apps
/// without a primary selection, it copies with Ctrl+C and reads the
/// clipboard back. Terminals are never sent Ctrl+C.
async fn capture_selection(
    clipboard: &mut Option<arboard::Clipboard>,
    use_wl: bool,
    terminal: bool,
) -> Selection {
    #[cfg(target_os = "linux")]
    {
        let primary = if use_wl { inject::wayland_primary_read() } else { inject::x11_primary_read() };
        if let Some(p) = primary.filter(|s| !s.trim().is_empty()) {
            return Selection { text: Some(p), original: None };
        }
    }
    if terminal {
        return Selection { text: None, original: None };
    }

    let original = crate::transform_clip_read(clipboard, use_wl);
    crate::transform_clip_write(clipboard, use_wl, "");
    if let Err(e) = inject::send_ctrl_c() {
        tracing::warn!("command mode: Ctrl+C failed ({e:#}); treating as no selection");
        crate::restore_clipboard_with(clipboard, use_wl, original);
        return Selection { text: None, original: None };
    }
    // Give the foreground app a moment to fill the clipboard.
    tokio::time::sleep(Duration::from_millis(220)).await;
    let copied = crate::transform_clip_read(clipboard, use_wl).unwrap_or_default();
    if copied.trim().is_empty() {
        // Nothing was selected: put the clipboard back right away.
        crate::restore_clipboard_with(clipboard, use_wl, original);
        return Selection { text: None, original: None };
    }
    Selection { text: Some(copied), original }
}

/// Runs a command to completion: transcribe the instruction, read the
/// selection, ask the model, paste the result.
pub async fn run(
    app: AppHandle,
    cfg: Config,
    wav: Vec<u8>,
    foreground_app: Option<String>,
    foreground_display: Option<String>,
    language: String,
    duration_ms: u64,
) {
    let started = Instant::now();

    // Bulbul's own windows have their own text routing (the scratchpad), which
    // command mode doesn't use.
    if foreground_app.as_deref().map(|a| a.to_lowercase().contains("bulbul")).unwrap_or(false) {
        crate::emit_status(&app, "idle", Some("Use it in another app".into()));
        return;
    }

    #[cfg(target_os = "linux")]
    let use_wl = inject::wayland_clipboard_available();
    #[cfg(not(target_os = "linux"))]
    let use_wl = false;
    let mut clipboard: Option<arboard::Clipboard> = if use_wl {
        None
    } else {
        match arboard::Clipboard::new() {
            Ok(c) => Some(c),
            Err(e) => {
                tracing::error!("command mode: clipboard open failed: {e:#}");
                crate::emit_status(&app, "error", Some(format!("Clipboard: {e:#}")));
                return;
            }
        }
    };

    let db = app.state::<AppState>().db.clone();
    let vocabulary: Vec<String> = db::list_dictionary(&db)
        .unwrap_or_default()
        .into_iter()
        .map(|e| e.to_word)
        .collect();

    let rl_app = app.clone();
    let on_rate_limit = move |secs: u64| {
        crate::emit_status(&rl_app, "rate_limited", Some(format!("Rate limited · {secs}s")));
    };
    let notify_rl: &groq::RetryNotify = &on_rate_limit;

    let terminal = is_terminal(foreground_app.as_deref());
    let (spoken, selection) = tokio::join!(
        stt::transcribe(&cfg, wav, &vocabulary, Some(notify_rl)),
        capture_selection(&mut clipboard, use_wl, terminal),
    );
    let Selection { text: selected, original } = selection;

    let instruction = match spoken {
        Ok(t) => {
            if let Some(from) = t.fell_back_from {
                crate::notify(
                    &app,
                    "Bulbul used Groq for this command",
                    &format!(
                        "{} didn't work, so this recording was sent to Groq. You can turn this off in Settings.",
                        stt::provider_label(from)
                    ),
                );
            }
            t.text
        }
        Err(e) => {
            tracing::error!("command mode: STT failed: {e:#}");
            crate::emit_status(&app, "error", Some(format!("STT: {e:#}")));
            crate::notify(&app, "Bulbul transcription failed", &format!("{e:#}"));
            crate::restore_clipboard_with(&mut clipboard, use_wl, original);
            return;
        }
    };
    let instruction = instruction.trim().to_string();
    if instruction.is_empty() || groq::is_likely_hallucination(&instruction) {
        crate::emit_status(&app, "idle", Some("No speech detected.".into()));
        crate::restore_clipboard_with(&mut clipboard, use_wl, original);
        return;
    }
    tracing::info!(
        "command: instruction={:?} selection={} chars",
        instruction.chars().take(120).collect::<String>(),
        selected.as_deref().map(|s| s.chars().count()).unwrap_or(0)
    );

    crate::emit_status(&app, "processing", Some("Working on it…".into()));
    let t_llm = Instant::now();
    let result = tokio::time::timeout(
        Duration::from_secs(120),
        groq::execute_command(&cfg, &instruction, selected.as_deref()),
    )
    .await;
    let result = match result {
        Ok(Ok(text)) => text,
        Ok(Err(e)) => {
            tracing::error!("command failed: {e:#}");
            crate::emit_status(&app, "error", Some(format!("{e:#}")));
            crate::notify(&app, "Bulbul command failed", &format!("{e:#}"));
            crate::restore_clipboard_with(&mut clipboard, use_wl, original);
            return;
        }
        Err(_) => {
            tracing::error!("command timed out");
            crate::emit_status(&app, "error", Some("The command took too long.".into()));
            crate::notify(&app, "Bulbul command failed", "The command took too long. Your text was not changed.");
            crate::restore_clipboard_with(&mut clipboard, use_wl, original);
            return;
        }
    };
    let llm_ms = t_llm.elapsed().as_millis() as u64;

    if result.trim().is_empty() {
        crate::emit_status(&app, "error", Some("The command returned nothing.".into()));
        crate::restore_clipboard_with(&mut clipboard, use_wl, original);
        return;
    }

    crate::emit_status(&app, "injecting", None);
    // The result is pasted as returned: no dictionary or snippet pass, no
    // dictation formatting. With text selected the paste replaces it.
    #[cfg(target_os = "linux")]
    let pasted: Result<(), String> = inject::inject_text(&result).map_err(|e| format!("{e:#}"));
    #[cfg(not(target_os = "linux"))]
    let pasted: Result<(), String> = if clipboard.is_some() {
        crate::clipboard_set_and_paste(&mut clipboard, &result).await
    } else {
        Err("The clipboard isn't available.".to_string())
    };
    if let Err(e) = pasted {
        tracing::error!("command inject failed: {e}");
        crate::emit_status(&app, "error", Some(e.clone()));
        crate::notify(&app, "Bulbul command failed", &e);
        crate::restore_clipboard_with(&mut clipboard, use_wl, original);
        return;
    }
    crate::emit_status(&app, "done", Some(result.clone()));

    // Keep a record so a long result is never lost. The stats count the words
    // the user SPOKE, not the generated text.
    if let Err(e) = db::log_command(
        &db,
        &instruction,
        &result,
        &language,
        foreground_app.clone(),
        foreground_display,
        duration_ms,
    ) {
        tracing::warn!("failed to log command: {e:#}");
    }

    if cfg.telemetry_enabled {
        crate::telemetry::track(
            "command_completed",
            serde_json::json!({
                "had_selection": selected.is_some(),
                "duration_bucket": crate::telemetry::duration_bucket(duration_ms),
                "llm_ms_bucket": crate::telemetry::duration_bucket(llm_ms),
                "terminal": terminal,
            }),
        );
    }
    tracing::info!(
        "perf-command: total={}ms llm={}ms | in={}c out={}c",
        started.elapsed().as_millis(),
        llm_ms,
        selected.as_deref().map(|s| s.chars().count()).unwrap_or(0),
        result.chars().count()
    );

    // Put the user's clipboard back after the paste has landed, but only if it
    // still holds our result (so nothing they copied meanwhile is lost).
    drop(clipboard);
    #[cfg(target_os = "linux")]
    if use_wl {
        if let Some(orig) = original {
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(250));
                let _ = inject::wayland_clipboard_write(&orig);
            });
        }
        return;
    }
    if let Some(orig) = original {
        let ours = result;
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(250));
            let Ok(mut cb) = arboard::Clipboard::new() else { return };
            if matches!(cb.get_text(), Ok(current) if current == ours) {
                let _ = cb.set_text(orig);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminals_are_recognised_by_their_program_name() {
        for t in ["WindowsTerminal.exe", "pwsh.exe", "cmd.exe", "gnome-terminal-server", "Alacritty", "com.apple.Terminal", "kitty"] {
            assert!(is_terminal(Some(t)), "{t}");
        }
        for t in ["notepad.exe", "chrome.exe", "code", "slack", "com.apple.Safari"] {
            assert!(!is_terminal(Some(t)), "{t}");
        }
        assert!(!is_terminal(None));
    }

    #[test]
    fn the_hotkey_is_empty_when_the_feature_is_off() {
        let mut cfg = Config::default();
        assert_eq!(hotkey(&cfg).key.as_deref(), Some("C"));
        assert!(hotkey(&cfg).shift && hotkey(&cfg).alt);
        cfg.command_mode_enabled = false;
        assert!(hotkey(&cfg).key.is_none());
        assert!(mouse_button(&cfg).is_none());
    }

    #[test]
    fn the_mouse_button_is_optional() {
        let mut cfg = Config::default();
        assert!(mouse_button(&cfg).is_none(), "no button by default");
        cfg.command_mouse_button = "back".into();
        assert_eq!(mouse_button(&cfg), Some(MouseButton::Back));
        cfg.command_mouse_button = "none".into();
        assert!(mouse_button(&cfg).is_none());
    }
}
