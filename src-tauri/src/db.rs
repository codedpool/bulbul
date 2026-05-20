use crate::config::{config_dir, CleanupMode};
use anyhow::{Context, Result};
use parking_lot::Mutex;
use rusqlite::{params, Connection};
use serde::Serialize;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS dictations (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    ts              INTEGER NOT NULL,
    raw_text        TEXT NOT NULL,
    cleaned_text    TEXT NOT NULL,
    mode            TEXT NOT NULL,
    language        TEXT NOT NULL,
    foreground_app  TEXT,
    duration_ms     INTEGER NOT NULL,
    word_count      INTEGER NOT NULL,
    fix_count       INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_dictations_ts ON dictations(ts DESC);
"#;

pub type Db = Arc<Mutex<Connection>>;

pub fn open() -> Result<Db> {
    let path = db_path()?;
    tracing::info!("opening sqlite db at {path:?}");
    let conn = Connection::open(&path).with_context(|| format!("opening {path:?}"))?;
    conn.execute_batch(SCHEMA).context("applying schema")?;
    Ok(Arc::new(Mutex::new(conn)))
}

fn db_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("bulbul.db"))
}

#[derive(Debug)]
pub struct LogEntry {
    pub raw_text: String,
    pub cleaned_text: String,
    pub mode: CleanupMode,
    pub language: String,
    pub foreground_app: Option<String>,
    pub duration_ms: u64,
}

pub fn log_dictation(db: &Db, entry: LogEntry) -> Result<()> {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let word_count = count_words(&entry.cleaned_text) as i64;
    let fix_count = count_fixes(&entry.raw_text, &entry.cleaned_text) as i64;
    let mode_str = match entry.mode {
        CleanupMode::Raw => "raw",
        CleanupMode::Clean => "clean",
        CleanupMode::Polished => "polished",
    };
    let conn = db.lock();
    conn.execute(
        "INSERT INTO dictations
            (ts, raw_text, cleaned_text, mode, language, foreground_app, duration_ms, word_count, fix_count)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        params![
            ts,
            entry.raw_text,
            entry.cleaned_text,
            mode_str,
            entry.language,
            entry.foreground_app,
            entry.duration_ms as i64,
            word_count,
            fix_count,
        ],
    )
    .context("inserting dictation")?;
    Ok(())
}

fn count_words(text: &str) -> usize {
    text.split_whitespace().filter(|w| !w.is_empty()).count()
}

/// Cheap symmetric-difference word count to approximate "fixes made".
/// Not a perfect edit distance, but accurate enough for stats display.
fn count_fixes(raw: &str, cleaned: &str) -> usize {
    let normalize = |s: &str| -> HashSet<String> {
        s.split_whitespace()
            .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase())
            .filter(|w| !w.is_empty())
            .collect()
    };
    let raw_set = normalize(raw);
    let clean_set = normalize(cleaned);
    let only_raw = raw_set.difference(&clean_set).count();
    let only_clean = clean_set.difference(&raw_set).count();
    only_raw + only_clean
}

#[derive(Debug, Serialize)]
pub struct DictationRow {
    pub id: i64,
    pub ts: i64,
    pub raw_text: String,
    pub cleaned_text: String,
    pub mode: String,
    pub language: String,
    pub foreground_app: Option<String>,
    pub duration_ms: i64,
    pub word_count: i64,
    pub fix_count: i64,
}

pub fn recent_dictations(db: &Db, limit: u32) -> Result<Vec<DictationRow>> {
    let conn = db.lock();
    let mut stmt = conn.prepare(
        "SELECT id, ts, raw_text, cleaned_text, mode, language, foreground_app,
                duration_ms, word_count, fix_count
         FROM dictations
         ORDER BY ts DESC
         LIMIT ?",
    )?;
    let rows = stmt
        .query_map([limit as i64], |r| {
            Ok(DictationRow {
                id: r.get(0)?,
                ts: r.get(1)?,
                raw_text: r.get(2)?,
                cleaned_text: r.get(3)?,
                mode: r.get(4)?,
                language: r.get(5)?,
                foreground_app: r.get(6)?,
                duration_ms: r.get(7)?,
                word_count: r.get(8)?,
                fix_count: r.get(9)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

#[derive(Debug, Serialize)]
pub struct HomeStats {
    pub total_words: i64,
    pub total_dictations: i64,
    pub total_fixes: i64,
    /// Average words-per-minute over the last 7 days of dictation.
    pub wpm_7d: f32,
    /// Consecutive days (including today) ending with at least one dictation.
    pub day_streak: i64,
}

pub fn home_stats(db: &Db) -> Result<HomeStats> {
    let conn = db.lock();
    let total_words: i64 = conn
        .query_row("SELECT COALESCE(SUM(word_count), 0) FROM dictations", [], |r| r.get(0))
        .unwrap_or(0);
    let total_dictations: i64 = conn
        .query_row("SELECT COUNT(*) FROM dictations", [], |r| r.get(0))
        .unwrap_or(0);
    let total_fixes: i64 = conn
        .query_row("SELECT COALESCE(SUM(fix_count), 0) FROM dictations", [], |r| r.get(0))
        .unwrap_or(0);

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let week_ago = now - 7 * 86_400;
    let (words_7d, ms_7d): (i64, i64) = conn
        .query_row(
            "SELECT COALESCE(SUM(word_count), 0), COALESCE(SUM(duration_ms), 0)
             FROM dictations WHERE ts >= ?",
            [week_ago],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap_or((0, 0));
    let wpm_7d = if ms_7d > 0 {
        (words_7d as f64 / (ms_7d as f64 / 60_000.0)) as f32
    } else {
        0.0
    };

    let day_streak = compute_streak(&conn);

    Ok(HomeStats {
        total_words,
        total_dictations,
        total_fixes,
        wpm_7d,
        day_streak,
    })
}

fn compute_streak(conn: &Connection) -> i64 {
    // Pull distinct local-date days (UTC for simplicity) from the activity log,
    // walk backwards from today counting consecutive days.
    let mut stmt = match conn.prepare(
        "SELECT DISTINCT date(ts, 'unixepoch') AS d
         FROM dictations
         ORDER BY d DESC",
    ) {
        Ok(s) => s,
        Err(_) => return 0,
    };
    let days: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .ok()
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default();

    if days.is_empty() {
        return 0;
    }

    // Today in UTC YYYY-MM-DD format.
    let today: String = match conn.query_row(
        "SELECT date('now')",
        [],
        |r| r.get::<_, String>(0),
    ) {
        Ok(d) => d,
        Err(_) => return 0,
    };

    // If the most recent dictation isn't today or yesterday, streak is 0.
    let yesterday: String = match conn.query_row(
        "SELECT date('now', '-1 day')",
        [],
        |r| r.get::<_, String>(0),
    ) {
        Ok(d) => d,
        Err(_) => return 0,
    };

    let mut iter = days.iter();
    let mut expected = if days[0] == today {
        today.clone()
    } else if days[0] == yesterday {
        yesterday.clone()
    } else {
        return 0;
    };

    let mut streak = 0i64;
    for d in iter.by_ref() {
        if *d == expected {
            streak += 1;
            // Move expected one day earlier. We use a SQLite roundtrip per step;
            // streaks are short so it's negligible.
            let next: String = match conn.query_row(
                "SELECT date(?, '-1 day')",
                [&expected],
                |r| r.get::<_, String>(0),
            ) {
                Ok(d) => d,
                Err(_) => break,
            };
            expected = next;
        } else {
            break;
        }
    }
    streak
}
