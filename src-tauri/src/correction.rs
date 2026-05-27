//! Correction memory (V3.1) — watch what the user changes about the text we
//! injected, so the cleanup model can learn their fixes over time.
//!
//! Phase 1 (this module today) is *observation only*: after injection we
//! snapshot the focused field via UIA, watch it until the user moves on or a
//! timeout fires, diff the result against what we pasted, and log any clean
//! correction we can extract. Nothing is stored or fed back yet — the point
//! is to learn which of the user's real apps are actually observable before
//! committing to schema and retrieval.

use crate::{db, uia, window_info};
use std::thread;
use std::time::{Duration, Instant};

/// How long to keep watching the field after injection.
const WATCH_SECS: u64 = 12;
/// Interval between field reads while the user is still in the field.
const POLL_MS: u64 = 800;
/// Skip fields larger than this (chars) — almost certainly a big document
/// where re-reading every poll is expensive and anchoring is unreliable.
const MAX_FIELD_CHARS: usize = 5000;

/// Fire-and-forget: spawn a background thread that watches the field the user
/// just dictated into and logs any correction they make. Never blocks the
/// dictation pipeline; all failures are silent (debug-logged).
pub fn watch_for_correction(injected: String, foreground_app: Option<String>, db: db::Db) {
    if injected.trim().is_empty() {
        return;
    }
    thread::spawn(move || {
        uia::with_com(|| run_watch(&injected, foreground_app.as_deref(), &db));
    });
}

fn run_watch(injected: &str, app: Option<&str>, db: &db::Db) {
    let Some(reader) = uia::Reader::new() else {
        return;
    };

    // Wait for the paste to land, then snapshot the focused element. Retry a
    // few times until our injected text shows up as a substring (clipboard
    // paste can lag a frame or two behind the Ctrl+V we sent). We keep the
    // element handle so every later read targets *this exact field*.
    let mut snapshot: Option<uia::Focused> = None;
    for _ in 0..6 {
        thread::sleep(Duration::from_millis(120));
        let Some(f) = reader.read_focused() else {
            continue;
        };
        if f.is_password {
            tracing::debug!("correction-watch: password field, skipping");
            return;
        }
        let matched = f.text.contains(injected);
        snapshot = Some(f);
        if matched {
            break;
        }
    }

    let Some(snapshot) = snapshot else {
        tracing::info!("correction-watch: field not readable via UIA (app={app:?} not observable)");
        return;
    };
    let before = snapshot.text.clone();
    if !before.contains(injected) {
        tracing::info!(
            "correction-watch: injected text not found in field after paste; skipping (field={} chars, injected={} chars)",
            before.chars().count(),
            injected.chars().count()
        );
        return;
    }
    if before.chars().count() > MAX_FIELD_CHARS {
        tracing::debug!("correction-watch: field too large ({} chars), skipping", before.chars().count());
        return;
    }

    // Capture the reference window only now that the paste is confirmed in the
    // field — the target definitely holds focus at this point, so we avoid
    // racing the overlay pill's z-order change right after injection.
    let start_hwnd = window_info::foreground_hwnd();

    // Poll until the user switches apps (foreground window changes = done
    // editing) or the watch window expires. Each read targets the *captured
    // element*, not whatever is focused now, so focus moving to a sibling
    // pane in the same window can't be mistaken for an edit. `latest` holds
    // the most recent successful read of our field.
    let deadline = Instant::now() + Duration::from_secs(WATCH_SECS);
    let mut latest = before.clone();
    loop {
        if Instant::now() >= deadline {
            break;
        }
        thread::sleep(Duration::from_millis(POLL_MS));
        if let Some(t) = reader.read_element_text(&snapshot.element) {
            latest = t;
        }
        if window_info::foreground_hwnd() != start_hwnd {
            // One last read so a fix made right before switching away counts.
            if let Some(t) = reader.read_element_text(&snapshot.element) {
                latest = t;
            }
            break;
        }
    }

    if latest == before {
        tracing::info!("correction-watch: no edits detected (app={app:?})");
        return;
    }

    match extract_correction(injected, &before, &latest) {
        Some(corrected) => {
            tracing::info!(
                "correction-watch: learned app={app:?}\n  injected:  {injected:?}\n  corrected: {corrected:?}"
            );
            if let Err(e) = db::add_correction(db, injected, &corrected, app) {
                tracing::warn!("correction-watch: failed to store correction: {e:#}");
            }
        }
        None => {
            tracing::info!(
                "correction-watch: field changed but no clean correction extractable (edited outside our span, or too divergent) app={app:?}"
            );
        }
    }
}

/// Given what we injected, the field's content right after the paste, and the
/// field's content after the user finished editing, try to isolate the edited
/// version of *our* injected span. Returns `None` when the change can't be
/// confidently attributed to a correction of our text.
fn extract_correction(injected: &str, before: &str, after: &str) -> Option<String> {
    // Locate our injected span inside the pre-edit field text. `find` gives a
    // byte offset; since `injected` is a real substring, all the slice
    // boundaries below land on char boundaries.
    let start = before.find(injected)?;
    let end = start + injected.len();
    let prefix = &before[..start];
    let suffix = &before[end..];

    // Common case: the user edited only within our span, so the text before
    // and after it is untouched. The middle of `after` is the correction.
    if after.starts_with(prefix)
        && after.ends_with(suffix)
        && after.len() >= prefix.len() + suffix.len()
    {
        let corrected = &after[prefix.len()..after.len() - suffix.len()];
        return finalize(injected, corrected);
    }

    // Our injection was the whole field (empty chat box, etc.): trust a
    // whole-field comparison, with the length guard in `finalize` protecting
    // against the user simply typing more.
    if prefix.trim().is_empty() && suffix.trim().is_empty() {
        return finalize(injected, after);
    }

    None
}

fn finalize(injected: &str, corrected: &str) -> Option<String> {
    let corrected = corrected.trim();
    let injected = injected.trim();
    if corrected.is_empty() || corrected == injected {
        return None;
    }
    // "Kept typing" rather than corrected: the result just extends what we
    // pasted. Not a correction.
    if corrected.starts_with(injected) {
        return None;
    }
    // A real correction stays close in length. Reject wholesale rewrites or
    // big additions (ratio measured against the original length).
    let inj_len = injected.chars().count().max(1) as f32;
    let cor_len = corrected.chars().count() as f32;
    if (cor_len - inj_len).abs() / inj_len > 0.6 {
        return None;
    }
    // A real correction keeps most of the original words and tweaks a few.
    // When a chat/search field is submitted it clears and shows a placeholder
    // ("Queue another message…", "Type a message") — that shares almost
    // nothing with what we injected, so it's a replacement, not a correction.
    if word_retention(injected, corrected) < 0.5 {
        return None;
    }
    Some(corrected.to_string())
}

/// Fraction of the injected text's words that survive into the corrected
/// text. High for genuine edits (a word or two changed), near zero when the
/// field was cleared and replaced by unrelated placeholder text.
fn word_retention(injected: &str, corrected: &str) -> f32 {
    fn words(s: &str) -> Vec<String> {
        s.split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(|w| w.to_lowercase())
            .collect()
    }
    let inj = words(injected);
    if inj.is_empty() {
        return 0.0;
    }
    let cor: std::collections::HashSet<String> = words(corrected).into_iter().collect();
    let kept = inj.iter().filter(|w| cor.contains(*w)).count();
    kept as f32 / inj.len() as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_in_span_edit() {
        // User fixed "their" -> "there" inside a larger field.
        let injected = "I think their going home";
        let before = "Hey. I think their going home";
        let after = "Hey. I think there going home";
        assert_eq!(
            extract_correction(injected, before, after).as_deref(),
            Some("I think there going home")
        );
    }

    #[test]
    fn whole_field_correction() {
        let injected = "lets meet at 5";
        assert_eq!(
            extract_correction(injected, injected, "let's meet at 5").as_deref(),
            Some("let's meet at 5")
        );
    }

    #[test]
    fn ignores_pure_continuation() {
        let injected = "see you soon";
        assert_eq!(
            extract_correction(injected, injected, "see you soon, take care now!"),
            None
        );
    }

    #[test]
    fn ignores_no_change() {
        let injected = "all good here";
        assert_eq!(extract_correction(injected, injected, "all good here"), None);
    }

    #[test]
    fn ignores_field_cleared_to_placeholder() {
        // Chat field submitted, then shows its input placeholder. Passes the
        // length guard but shares no words with what we injected.
        let injected = "Can you give me the sentences to type and try?";
        assert_eq!(
            extract_correction(injected, injected, "Queue another message"),
            None
        );
    }

    #[test]
    fn keeps_single_word_fix() {
        // The marquee case: one misheard word fixed, everything else intact.
        let injected = "Let me check if it works on GLaud";
        assert_eq!(
            extract_correction(injected, injected, "Let me check if it works on Claude").as_deref(),
            Some("Let me check if it works on Claude")
        );
    }

    #[test]
    fn ignores_wholesale_rewrite() {
        let injected = "ok";
        assert_eq!(
            extract_correction(injected, injected, "this is a completely different much longer message"),
            None
        );
    }
}
