use crate::config::{config_dir, CleanupMode};
use anyhow::{Context, Result};
use parking_lot::Mutex;
use regex::{Regex, RegexBuilder};
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

CREATE TABLE IF NOT EXISTS dictionary (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    from_word       TEXT NOT NULL,
    to_word         TEXT NOT NULL,
    case_sensitive  INTEGER NOT NULL DEFAULT 0,
    hit_count       INTEGER NOT NULL DEFAULT 0,
    created_at      INTEGER NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_dictionary_from
    ON dictionary(from_word COLLATE NOCASE);

CREATE TABLE IF NOT EXISTS voice_profile (
    id                  INTEGER PRIMARY KEY CHECK (id = 1),
    voice_narrative     TEXT,
    peak_narrative      TEXT,
    last_generated_at   INTEGER,
    last_word_count     INTEGER NOT NULL DEFAULT 0
);
INSERT OR IGNORE INTO voice_profile (id, last_word_count) VALUES (1, 0);

CREATE TABLE IF NOT EXISTS snippets (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    trigger     TEXT NOT NULL,
    expansion   TEXT NOT NULL,
    hit_count   INTEGER NOT NULL DEFAULT 0,
    created_at  INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_snippets_trigger
    ON snippets(trigger COLLATE NOCASE);

CREATE TABLE IF NOT EXISTS transforms (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    name            TEXT NOT NULL,
    description     TEXT NOT NULL DEFAULT '',
    system_prompt   TEXT NOT NULL,
    is_default      INTEGER NOT NULL DEFAULT 0,
    sort_order      INTEGER NOT NULL DEFAULT 0,
    hit_count       INTEGER NOT NULL DEFAULT 0,
    created_at      INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS notes (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    title       TEXT NOT NULL DEFAULT '',
    body        TEXT NOT NULL DEFAULT '',
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_notes_updated ON notes(updated_at DESC);

CREATE TABLE IF NOT EXISTS corrections (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    ts              INTEGER NOT NULL,
    injected        TEXT NOT NULL,
    corrected       TEXT NOT NULL,
    foreground_app  TEXT,
    hit_count       INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_corrections_ts ON corrections(ts DESC);

CREATE TABLE IF NOT EXISTS correction_dismissals (
    from_word   TEXT NOT NULL,
    to_word     TEXT NOT NULL,
    PRIMARY KEY (from_word, to_word)
);
"#;

const DEFAULT_TRANSFORMS: &[(&str, &str, &str)] = &[
    (
        "Polish",
        "Improve clarity and conciseness",
        "You are a writing editor. Polish the user's text:\n\
- Fix grammar, spelling, and punctuation errors.\n\
- Improve flow and clarity.\n\
- Preserve the original meaning, tone, and rough length \u{2014} do not add new ideas.\n\
- Match the original register (casual stays casual, formal stays formal).\n\
\n\
Return ONLY the polished text. No preamble, no quotes around the output, no commentary.",
    ),
    (
        "Prompt Engineer",
        "Constructs optimal LLM prompts",
        "You are a prompt engineer. Rewrite the user's text into a clear, well-structured prompt for a large language model.\n\
- Open with the role / task in one sentence.\n\
- Add explicit instructions, constraints, and output format if implied.\n\
- Preserve every concrete detail the user provided.\n\
- Use sections (\"Task:\", \"Constraints:\", \"Output:\") only if it improves clarity.\n\
\n\
Return ONLY the rewritten prompt. No preamble, no commentary.",
    ),
    (
        "Make Formal",
        "Switches to a professional tone",
        "Rewrite the user's text in a formal, professional tone.\n\
- Use full sentences, proper grammar, conventional punctuation.\n\
- Avoid contractions, slang, and filler.\n\
- Preserve the meaning and approximate length.\n\
\n\
Return ONLY the rewritten text.",
    ),
    (
        "Make Casual",
        "Loosens the tone, keeps the meaning",
        "Rewrite the user's text in a casual, friendly tone, as if talking to a colleague.\n\
- Use contractions where natural.\n\
- Keep it concise and human.\n\
- Do not add jokes or new ideas.\n\
- Preserve every fact and intent.\n\
\n\
Return ONLY the rewritten text.",
    ),
    (
        "Bullet Points",
        "Convert prose into a clean bulleted list",
        "Convert the user's text into a clean bulleted list.\n\
- Each bullet is one clear point.\n\
- Preserve every fact and the original order.\n\
- Use dash bullets (\"- \"), one per line.\n\
- No nested bullets unless the source clearly has sub-points.\n\
\n\
Return ONLY the bulleted list. No preamble.",
    ),
];

const STOP_WORDS: &[&str] = &[
    "the", "a", "an", "and", "or", "but", "is", "are", "was", "were", "be", "been",
    "being", "have", "has", "had", "do", "does", "did", "will", "would", "could",
    "should", "may", "might", "must", "shall", "can", "of", "in", "on", "at", "to",
    "for", "with", "by", "from", "up", "about", "into", "through", "during", "before",
    "after", "above", "below", "between", "i", "you", "he", "she", "it", "we", "they",
    "me", "him", "her", "us", "them", "my", "your", "his", "its", "our", "their",
    "this", "that", "these", "those", "what", "which", "who", "whom", "whose", "where",
    "when", "why", "how", "all", "each", "every", "both", "few", "more", "most",
    "other", "some", "such", "no", "not", "only", "own", "same", "so", "than", "too",
    "very", "just", "as", "if", "any", "yes", "well", "okay", "ok", "yeah", "im",
    "youre", "theyre", "weve", "ive", "dont", "doesnt", "didnt", "wont", "cant",
    "isnt", "arent", "wasnt", "werent", "thats", "theres", "heres", "let", "lets",
    "go", "going", "get", "got", "really", "actually", "basically", "kind", "sort",
];

const DEFAULT_DICTIONARY: &[(&str, &str)] = &[
    ("groq", "Groq"),
    ("example", "app"),
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

pub type Db = Arc<Mutex<Connection>>;

pub fn open() -> Result<Db> {
    let path = db_path()?;
    tracing::info!("opening sqlite db at {path:?}");
    let conn = Connection::open(&path).with_context(|| format!("opening {path:?}"))?;
    conn.execute_batch(SCHEMA).context("applying schema")?;
    seed_default_dictionary(&conn).context("seeding dictionary defaults")?;
    seed_default_transforms(&conn).context("seeding transform defaults")?;
    Ok(Arc::new(Mutex::new(conn)))
}

fn seed_default_transforms(conn: &Connection) -> Result<()> {
    let existing: i64 = conn
        .query_row("SELECT COUNT(*) FROM transforms", [], |r| r.get(0))
        .unwrap_or(0);
    if existing > 0 {
        return Ok(());
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    for (idx, (name, desc, prompt)) in DEFAULT_TRANSFORMS.iter().enumerate() {
        let is_default = if idx == 0 { 1 } else { 0 };
        conn.execute(
            "INSERT INTO transforms
                (name, description, system_prompt, is_default, sort_order, hit_count, created_at)
             VALUES (?, ?, ?, ?, ?, 0, ?)",
            params![name, desc, prompt, is_default, idx as i64, now],
        )?;
    }
    tracing::info!("seeded {} default transforms", DEFAULT_TRANSFORMS.len());
    Ok(())
}

fn seed_default_dictionary(conn: &Connection) -> Result<()> {
    let existing: i64 = conn
        .query_row("SELECT COUNT(*) FROM dictionary", [], |r| r.get(0))
        .unwrap_or(0);
    if existing > 0 {
        return Ok(());
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    for (from, to) in DEFAULT_DICTIONARY {
        conn.execute(
            "INSERT OR IGNORE INTO dictionary
                (from_word, to_word, case_sensitive, hit_count, created_at)
             VALUES (?, ?, 0, 0, ?)",
            params![from, to, now],
        )?;
    }
    tracing::info!("seeded {} default dictionary entries", DEFAULT_DICTIONARY.len());
    Ok(())
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

/// Atomic end-of-dictation write: one transaction commits the activity-log
/// INSERT and all hit-counter UPDATEs together. SQLite does one `fsync`
/// instead of three, saving ~30ms on Windows. Stronger consistency than the
/// previous three-separate-calls path because partial failure is impossible:
/// either the whole dictation is recorded or none of it is.
pub fn log_dictation_with_hits(
    db: &Db,
    entry: LogEntry,
    dict_hits: &[(i64, i64)],
    snip_hits: &[(i64, i64)],
) -> Result<()> {
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
    let tx = conn
        .unchecked_transaction()
        .context("starting batched dictation transaction")?;
    tx.execute(
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
    .context("inserting dictation in batch")?;
    for (id, delta) in dict_hits {
        tx.execute(
            "UPDATE dictionary SET hit_count = hit_count + ? WHERE id = ?",
            params![*delta, *id],
        )
        .context("bumping dictionary hit in batch")?;
    }
    for (id, delta) in snip_hits {
        tx.execute(
            "UPDATE snippets SET hit_count = hit_count + ? WHERE id = ?",
            params![*delta, *id],
        )
        .context("bumping snippet hit in batch")?;
    }
    tx.commit().context("committing batched dictation")?;
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

/// Pull recent (raw, cleaned) pairs that the cleanup pipeline can use as
/// few-shot personalization examples. Filters by foreground app + mode so
/// the model only sees stylistically-comparable history. Falls back to
/// mode-only when no app match exists, so a brand-new user in a new app
/// still gets weak personalization rather than none. Each text is read
/// verbatim — caller is responsible for truncation/formatting.
pub fn style_memory(
    db: &Db,
    foreground_app: Option<&str>,
    mode: &str,
    k: u32,
) -> Result<Vec<(String, String)>> {
    let conn = db.lock();
    if let Some(app) = foreground_app {
        let mut stmt = conn.prepare(
            "SELECT raw_text, cleaned_text FROM dictations
             WHERE foreground_app = ?1 AND mode = ?2
               AND TRIM(raw_text) != '' AND TRIM(cleaned_text) != ''
             ORDER BY ts DESC LIMIT ?3",
        )?;
        let rows: Vec<(String, String)> = stmt
            .query_map(params![app, mode, k as i64], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if !rows.is_empty() {
            return Ok(rows);
        }
    }
    let mut stmt = conn.prepare(
        "SELECT raw_text, cleaned_text FROM dictations
         WHERE mode = ?1
           AND TRIM(raw_text) != '' AND TRIM(cleaned_text) != ''
         ORDER BY ts DESC LIMIT ?2",
    )?;
    let rows: Vec<(String, String)> = stmt
        .query_map(params![mode, k as i64], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Most recent corrections to keep. Correction memory is recency-biased —
/// old fixes for vocabulary the user no longer uses age out automatically.
const MAX_CORRECTIONS: i64 = 200;

/// Persist a `{injected, corrected}` pair captured by the correction watcher.
/// Exact-duplicate pairs refresh their timestamp and bump a hit counter
/// instead of piling up, so a fix the user keeps making floats to the top.
/// The table is pruned to the most recent `MAX_CORRECTIONS` on every insert.
pub fn add_correction(
    db: &Db,
    injected: &str,
    corrected: &str,
    foreground_app: Option<&str>,
) -> Result<()> {
    let injected = injected.trim();
    let corrected = corrected.trim();
    if injected.is_empty() || corrected.is_empty() || injected == corrected {
        return Ok(());
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let conn = db.lock();
    let updated = conn.execute(
        "UPDATE corrections SET ts = ?, hit_count = hit_count + 1
         WHERE injected = ? AND corrected = ?",
        params![now, injected, corrected],
    )?;
    if updated == 0 {
        conn.execute(
            "INSERT INTO corrections (ts, injected, corrected, foreground_app, hit_count)
             VALUES (?, ?, ?, ?, 0)",
            params![now, injected, corrected, foreground_app],
        )?;
    }
    conn.execute(
        "DELETE FROM corrections WHERE id NOT IN
            (SELECT id FROM corrections ORDER BY ts DESC LIMIT ?)",
        params![MAX_CORRECTIONS],
    )?;
    Ok(())
}

/// Pick the corrections most relevant to the text the user just dictated, for
/// injection into the cleanup prompt. Relevance is matched against the words
/// that *actually changed* in each correction (the symmetric difference of
/// its before/after tokens), not every word it contains. A `"GLaud"→"Claude"`
/// fix is only relevant when the new transcript itself contains "glaud" or
/// "claude" — it shouldn't fire just because both mention "check". This keeps
/// recurring-error fixes (the whole point) while ignoring incidental overlap
/// on common words. Ties break toward the more frequently repeated fix.
///
/// Currently unused: the few-shot apply path was disabled after it caused the
/// cleanup model to echo example text. Retained for the safe apply redesign.
#[allow(dead_code)]
pub fn relevant_corrections(
    db: &Db,
    transcript: &str,
    k: u32,
) -> Result<Vec<(String, String)>> {
    let conn = db.lock();
    let mut stmt = conn.prepare(
        "SELECT injected, corrected, hit_count FROM corrections
         ORDER BY ts DESC LIMIT 100",
    )?;
    let rows: Vec<(String, String, i64)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let trans_tokens: HashSet<String> = tokenize_words(transcript)
        .into_iter()
        .filter(|w| is_meaningful(w))
        .collect();
    if trans_tokens.is_empty() {
        return Ok(Vec::new());
    }

    let mut scored: Vec<(i64, i64, (String, String))> = rows
        .into_iter()
        .map(|(inj, cor, hits)| {
            let inj_tokens: HashSet<String> = tokenize_words(&inj).into_iter().collect();
            let cor_tokens: HashSet<String> = tokenize_words(&cor).into_iter().collect();
            // Only the words that changed in this correction can make it
            // relevant to a new transcript.
            let overlap = inj_tokens
                .symmetric_difference(&cor_tokens)
                .filter(|t| {
                    let w = t.as_str();
                    is_meaningful(w) && trans_tokens.contains(w)
                })
                .count() as i64;
            (overlap, hits, (inj, cor))
        })
        .filter(|(overlap, _, _)| *overlap > 0)
        .collect();
    // Most changed-word matches first, then most-repeated correction.
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
    Ok(scored
        .into_iter()
        .take(k as usize)
        .map(|(_, _, pair)| pair)
        .collect())
}

#[derive(Debug, Serialize)]
pub struct CorrectionSuggestion {
    pub from_word: String,
    pub to_word: String,
    pub count: i64,
}

/// If `injected` and `corrected` differ by exactly one whitespace-delimited
/// word (ignoring surrounding punctuation), return that `(from, to)` swap.
/// This is the safe, dictionary-shaped subset of corrections — a clean
/// single-word fix like "GLaud" → "Claude". Multi-word or structural edits
/// return `None` and never become suggestions.
fn extract_word_substitution(injected: &str, corrected: &str) -> Option<(String, String)> {
    let inj: Vec<&str> = injected.split_whitespace().collect();
    let cor: Vec<&str> = corrected.split_whitespace().collect();
    if inj.is_empty() || inj.len() != cor.len() {
        return None;
    }
    fn strip(w: &str) -> &str {
        w.trim_matches(|c: char| !c.is_alphanumeric())
    }
    let mut diff: Option<(String, String)> = None;
    for (a, b) in inj.iter().zip(cor.iter()) {
        let (sa, sb) = (strip(a), strip(b));
        if sa == sb {
            continue;
        }
        if diff.is_some() {
            return None; // more than one word changed — not a clean swap
        }
        diff = Some((sa.to_string(), sb.to_string()));
    }
    let (from, to) = diff?;
    if from.is_empty() || to.is_empty() || from.chars().count() > 40 || to.chars().count() > 40 {
        return None;
    }
    Some((from, to))
}

/// Distinct single-word fixes the user has made, as dictionary suggestions.
/// Each captured correction is reduced to its one changed word (if it is a
/// clean swap); identical swaps are grouped and counted. Pairs already in the
/// dictionary or previously dismissed are excluded. Sorted by how often the
/// fix recurred (most-repeated first), capped to keep the list reviewable.
pub fn correction_suggestions(db: &Db) -> Result<Vec<CorrectionSuggestion>> {
    let conn = db.lock();
    let dict: HashSet<String> = {
        let mut s = conn.prepare("SELECT from_word FROM dictionary")?;
        let set = s
            .query_map([], |r| r.get::<_, String>(0))?
            .filter_map(|r| r.ok())
            .map(|w| w.to_lowercase())
            .collect();
        set
    };
    let dismissed: HashSet<(String, String)> = {
        let mut s = conn.prepare("SELECT from_word, to_word FROM correction_dismissals")?;
        let set = s
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .filter_map(|r| r.ok())
            .collect();
        set
    };
    let rows: Vec<(String, String)> = {
        let mut s = conn.prepare("SELECT injected, corrected FROM corrections ORDER BY ts DESC")?;
        let v = s
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        v
    };

    // Preserve recency order of first appearance so ties sort newest-first.
    let mut order: Vec<(String, String)> = Vec::new();
    let mut map: std::collections::HashMap<(String, String), CorrectionSuggestion> =
        std::collections::HashMap::new();
    for (inj, cor) in rows {
        let Some((from, to)) = extract_word_substitution(&inj, &cor) else {
            continue;
        };
        let from_lower = from.to_lowercase();
        let to_lower = to.to_lowercase();
        if dict.contains(&from_lower) {
            continue;
        }
        let key = (from_lower, to_lower);
        if dismissed.contains(&key) {
            continue;
        }
        match map.get_mut(&key) {
            Some(existing) => existing.count += 1,
            None => {
                order.push(key.clone());
                map.insert(
                    key,
                    CorrectionSuggestion {
                        from_word: from,
                        to_word: to,
                        count: 1,
                    },
                );
            }
        }
    }
    let mut out: Vec<CorrectionSuggestion> =
        order.into_iter().filter_map(|k| map.remove(&k)).collect();
    out.sort_by(|a, b| b.count.cmp(&a.count)); // stable: keeps recency within ties
    out.truncate(20);
    Ok(out)
}

/// Remember that the user dismissed a suggested correction so it stops being
/// offered. Stored case-insensitively to match `correction_suggestions`.
pub fn dismiss_correction_suggestion(db: &Db, from_word: &str, to_word: &str) -> Result<()> {
    let conn = db.lock();
    conn.execute(
        "INSERT OR IGNORE INTO correction_dismissals (from_word, to_word) VALUES (?, ?)",
        params![from_word.to_lowercase(), to_word.to_lowercase()],
    )?;
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct CorrectionHistoryRow {
    pub injected: String,
    pub corrected: String,
    pub foreground_app: Option<String>,
    pub ts: i64,
    pub hit_count: i64,
}

/// Full correction history (most recent first) for the Insights view — every
/// hand-fix the watcher captured, not just the ones distilled into dictionary
/// suggestions.
pub fn list_corrections(db: &Db, limit: u32) -> Result<Vec<CorrectionHistoryRow>> {
    let conn = db.lock();
    let mut stmt = conn.prepare(
        "SELECT injected, corrected, foreground_app, ts, hit_count
         FROM corrections ORDER BY ts DESC LIMIT ?1",
    )?;
    let rows = stmt
        .query_map(params![limit], |r| {
            Ok(CorrectionHistoryRow {
                injected: r.get(0)?,
                corrected: r.get(1)?,
                foreground_app: r.get(2)?,
                ts: r.get(3)?,
                hit_count: r.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Count dictations logged at or after `ts`. Used to decide when enough new
/// dictations have accrued since the last voice-profile generation to warrant
/// an automatic refresh.
pub fn dictations_since(db: &Db, ts: i64) -> Result<i64> {
    let conn = db.lock();
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM dictations WHERE ts >= ?1",
        params![ts],
        |r| r.get(0),
    )?;
    Ok(n)
}

pub fn recent_dictations(db: &Db, limit: u32, offset: u32) -> Result<Vec<DictationRow>> {
    let conn = db.lock();
    let mut stmt = conn.prepare(
        "SELECT id, ts, raw_text, cleaned_text, mode, language, foreground_app,
                duration_ms, word_count, fix_count
         FROM dictations
         ORDER BY ts DESC
         LIMIT ? OFFSET ?",
    )?;
    let rows = stmt
        .query_map([limit as i64, offset as i64], |r| {
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

#[derive(Debug, Serialize, serde::Deserialize, Clone)]
pub struct DictionaryEntry {
    pub id: i64,
    pub from_word: String,
    pub to_word: String,
    pub case_sensitive: bool,
    pub hit_count: i64,
    pub created_at: i64,
}

pub fn list_dictionary(db: &Db) -> Result<Vec<DictionaryEntry>> {
    let conn = db.lock();
    let mut stmt = conn.prepare(
        "SELECT id, from_word, to_word, case_sensitive, hit_count, created_at
         FROM dictionary
         ORDER BY hit_count DESC, from_word COLLATE NOCASE ASC",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok(DictionaryEntry {
                id: r.get(0)?,
                from_word: r.get(1)?,
                to_word: r.get(2)?,
                case_sensitive: r.get::<_, i64>(3)? != 0,
                hit_count: r.get(4)?,
                created_at: r.get(5)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn add_dictionary_entry(
    db: &Db,
    from_word: &str,
    to_word: &str,
    case_sensitive: bool,
) -> Result<DictionaryEntry> {
    let from = from_word.trim();
    let to = to_word.trim();
    if from.is_empty() {
        return Err(anyhow::anyhow!("from-word cannot be empty"));
    }
    if to.is_empty() {
        return Err(anyhow::anyhow!("to-word cannot be empty"));
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let conn = db.lock();
    conn.execute(
        "INSERT INTO dictionary
            (from_word, to_word, case_sensitive, hit_count, created_at)
         VALUES (?, ?, ?, 0, ?)",
        params![from, to, case_sensitive as i64, now],
    )
    .context("inserting dictionary entry")?;
    let id = conn.last_insert_rowid();
    Ok(DictionaryEntry {
        id,
        from_word: from.to_string(),
        to_word: to.to_string(),
        case_sensitive,
        hit_count: 0,
        created_at: now,
    })
}

pub fn update_dictionary_entry(
    db: &Db,
    id: i64,
    from_word: &str,
    to_word: &str,
    case_sensitive: bool,
) -> Result<()> {
    let from = from_word.trim();
    let to = to_word.trim();
    if from.is_empty() || to.is_empty() {
        return Err(anyhow::anyhow!("from-word and to-word are required"));
    }
    let conn = db.lock();
    let affected = conn.execute(
        "UPDATE dictionary SET from_word = ?, to_word = ?, case_sensitive = ? WHERE id = ?",
        params![from, to, case_sensitive as i64, id],
    )?;
    if affected == 0 {
        return Err(anyhow::anyhow!("no dictionary entry with id {id}"));
    }
    Ok(())
}

pub fn delete_dictionary_entry(db: &Db, id: i64) -> Result<()> {
    let conn = db.lock();
    let affected = conn.execute("DELETE FROM dictionary WHERE id = ?", params![id])?;
    if affected == 0 {
        return Err(anyhow::anyhow!("no dictionary entry with id {id}"));
    }
    Ok(())
}

/// Apply dictionary substitutions to `text` using all entries. Returns the
/// transformed text plus a list of `(entry_id, hit_count_delta)` for callers
/// to persist via `bump_dictionary_hits`. Hits are only counted when the
/// match's casing actually differs from the canonical `to_word`, so e.g. a
/// transcript already spelling "Groq" correctly doesn't inflate the counter.
/// Pre-compiled dictionary + snippet regex caches. Compiling 24+ regexes per
/// dictation costs ~50–60ms; with these caches the second-and-later dictation
/// runs the regex pass in single-digit ms. Caches lazily populate on first
/// use and are invalidated whenever the underlying CRUD command runs.
pub struct RegexCache {
    dict: Mutex<Option<Vec<CompiledDictEntry>>>,
    snip: Mutex<Option<Vec<CompiledSnippet>>>,
}

struct CompiledDictEntry {
    id: i64,
    to_word: String,
    re: Regex,
}

struct CompiledSnippet {
    id: i64,
    expansion: String,
    re: Regex,
}

impl Default for RegexCache {
    fn default() -> Self {
        Self {
            dict: Mutex::new(None),
            snip: Mutex::new(None),
        }
    }
}

impl RegexCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn invalidate_dictionary(&self) {
        *self.dict.lock() = None;
    }

    pub fn invalidate_snippets(&self) {
        *self.snip.lock() = None;
    }

    /// Apply dictionary substitutions using the cached compiled regexes.
    /// On first call (or after invalidation) the cache rebuilds from the
    /// database; subsequent calls reuse the compiled regexes.
    pub fn apply_dictionary(&self, db: &Db, text: &str) -> (String, Vec<(i64, i64)>) {
        let mut guard = self.dict.lock();
        if guard.is_none() {
            let entries = match list_dictionary(db) {
                Ok(e) => e,
                Err(e) => {
                    tracing::warn!("could not list dictionary for substitution: {e:#}");
                    return (text.to_string(), Vec::new());
                }
            };
            let compiled: Vec<CompiledDictEntry> = entries
                .into_iter()
                .filter_map(|entry| {
                    let pattern = format!(r"\b{}\b", regex::escape(&entry.from_word));
                    RegexBuilder::new(&pattern)
                        .case_insensitive(!entry.case_sensitive)
                        .build()
                        .map_err(|e| {
                            tracing::warn!(
                                "invalid dictionary regex for {:?}: {e:#}",
                                entry.from_word
                            );
                        })
                        .ok()
                        .map(|re| CompiledDictEntry {
                            id: entry.id,
                            to_word: entry.to_word,
                            re,
                        })
                })
                .collect();
            *guard = Some(compiled);
        }
        let entries = guard.as_ref().unwrap();

        let mut working = text.to_string();
        let mut hits: Vec<(i64, i64)> = Vec::new();
        for entry in entries {
            let mut count = 0i64;
            let to_word = &entry.to_word;
            let replaced = entry.re.replace_all(&working, |caps: &regex::Captures| {
                let matched = caps.get(0).map(|m| m.as_str()).unwrap_or("");
                if matched != to_word.as_str() {
                    count += 1;
                }
                to_word.clone()
            });
            if count > 0 {
                hits.push((entry.id, count));
                working = replaced.into_owned();
            }
        }
        (working, hits)
    }

    /// Apply snippet expansions using the cached compiled regexes.
    pub fn apply_snippets(&self, db: &Db, text: &str) -> (String, Vec<(i64, i64)>) {
        let mut guard = self.snip.lock();
        if guard.is_none() {
            let snippets = match list_snippets(db) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("could not list snippets: {e:#}");
                    return (text.to_string(), Vec::new());
                }
            };
            let compiled: Vec<CompiledSnippet> = snippets
                .into_iter()
                .filter_map(|snip| {
                    let pattern = format!(r"\b{}\b", regex::escape(&snip.trigger));
                    RegexBuilder::new(&pattern)
                        .case_insensitive(true)
                        .build()
                        .map_err(|e| {
                            tracing::warn!("invalid snippet regex for {:?}: {e:#}", snip.trigger);
                        })
                        .ok()
                        .map(|re| CompiledSnippet {
                            id: snip.id,
                            expansion: snip.expansion,
                            re,
                        })
                })
                .collect();
            *guard = Some(compiled);
        }
        let snippets = guard.as_ref().unwrap();

        let mut working = text.to_string();
        let mut hits: Vec<(i64, i64)> = Vec::new();
        for snip in snippets {
            let mut count = 0i64;
            let expansion = &snip.expansion;
            let replaced = snip.re.replace_all(&working, |_caps: &regex::Captures| {
                count += 1;
                expansion.clone()
            });
            if count > 0 {
                hits.push((snip.id, count));
                working = replaced.into_owned();
            }
        }
        (working, hits)
    }

    /// Pre-compile both caches from the database so the first dictation of a
    /// session doesn't pay the ~50–120ms compile cost. Running the apply pass
    /// over an empty string just triggers the lazy build and returns at once.
    /// Safe to call on a background thread at boot.
    pub fn warm(&self, db: &Db) {
        let _ = self.apply_dictionary(db, "");
        let _ = self.apply_snippets(db, "");
    }
}

#[derive(Debug, Serialize, serde::Deserialize, Clone)]
pub struct Snippet {
    pub id: i64,
    pub trigger: String,
    pub expansion: String,
    pub hit_count: i64,
    pub created_at: i64,
}

pub fn list_snippets(db: &Db) -> Result<Vec<Snippet>> {
    let conn = db.lock();
    let mut stmt = conn.prepare(
        "SELECT id, trigger, expansion, hit_count, created_at
         FROM snippets
         ORDER BY hit_count DESC, trigger COLLATE NOCASE ASC",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok(Snippet {
                id: r.get(0)?,
                trigger: r.get(1)?,
                expansion: r.get(2)?,
                hit_count: r.get(3)?,
                created_at: r.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn add_snippet(db: &Db, trigger: &str, expansion: &str) -> Result<Snippet> {
    let trig = trigger.trim();
    let exp = expansion.trim();
    if trig.is_empty() {
        return Err(anyhow::anyhow!("trigger cannot be empty"));
    }
    if exp.is_empty() {
        return Err(anyhow::anyhow!("expansion cannot be empty"));
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let conn = db.lock();
    conn.execute(
        "INSERT INTO snippets (trigger, expansion, hit_count, created_at)
         VALUES (?, ?, 0, ?)",
        params![trig, exp, now],
    )?;
    let id = conn.last_insert_rowid();
    Ok(Snippet {
        id,
        trigger: trig.to_string(),
        expansion: exp.to_string(),
        hit_count: 0,
        created_at: now,
    })
}

pub fn update_snippet(db: &Db, id: i64, trigger: &str, expansion: &str) -> Result<()> {
    let trig = trigger.trim();
    let exp = expansion.trim();
    if trig.is_empty() || exp.is_empty() {
        return Err(anyhow::anyhow!("trigger and expansion are required"));
    }
    let conn = db.lock();
    let affected = conn.execute(
        "UPDATE snippets SET trigger = ?, expansion = ? WHERE id = ?",
        params![trig, exp, id],
    )?;
    if affected == 0 {
        return Err(anyhow::anyhow!("no snippet with id {id}"));
    }
    Ok(())
}

pub fn delete_snippet(db: &Db, id: i64) -> Result<()> {
    let conn = db.lock();
    let affected = conn.execute("DELETE FROM snippets WHERE id = ?", params![id])?;
    if affected == 0 {
        return Err(anyhow::anyhow!("no snippet with id {id}"));
    }
    Ok(())
}


#[derive(Debug, Serialize, serde::Deserialize, Clone)]
pub struct Transform {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub system_prompt: String,
    pub is_default: bool,
    pub sort_order: i64,
    pub hit_count: i64,
    pub created_at: i64,
}

pub fn list_transforms(db: &Db) -> Result<Vec<Transform>> {
    let conn = db.lock();
    let mut stmt = conn.prepare(
        "SELECT id, name, description, system_prompt, is_default, sort_order, hit_count, created_at
         FROM transforms
         ORDER BY sort_order ASC, id ASC",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok(Transform {
                id: r.get(0)?,
                name: r.get(1)?,
                description: r.get(2)?,
                system_prompt: r.get(3)?,
                is_default: r.get::<_, i64>(4)? != 0,
                sort_order: r.get(5)?,
                hit_count: r.get(6)?,
                created_at: r.get(7)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

#[allow(dead_code)]
pub fn get_transform(db: &Db, id: i64) -> Result<Transform> {
    let conn = db.lock();
    let row = conn.query_row(
        "SELECT id, name, description, system_prompt, is_default, sort_order, hit_count, created_at
         FROM transforms WHERE id = ?",
        [id],
        |r| {
            Ok(Transform {
                id: r.get(0)?,
                name: r.get(1)?,
                description: r.get(2)?,
                system_prompt: r.get(3)?,
                is_default: r.get::<_, i64>(4)? != 0,
                sort_order: r.get(5)?,
                hit_count: r.get(6)?,
                created_at: r.get(7)?,
            })
        },
    )?;
    Ok(row)
}

#[allow(dead_code)] // No callers since the default-transform hotkey was
                    // removed, but the is_default DB flag is still written
                    // by set_default_transform and Transforms UI; this
                    // getter stays available for future readers (e.g. a
                    // pill wand button, app-aware prompt selection).
pub fn get_default_transform(db: &Db) -> Result<Transform> {
    let conn = db.lock();
    let row = conn.query_row(
        "SELECT id, name, description, system_prompt, is_default, sort_order, hit_count, created_at
         FROM transforms
         ORDER BY is_default DESC, sort_order ASC
         LIMIT 1",
        [],
        |r| {
            Ok(Transform {
                id: r.get(0)?,
                name: r.get(1)?,
                description: r.get(2)?,
                system_prompt: r.get(3)?,
                is_default: r.get::<_, i64>(4)? != 0,
                sort_order: r.get(5)?,
                hit_count: r.get(6)?,
                created_at: r.get(7)?,
            })
        },
    )?;
    Ok(row)
}

pub fn add_transform(
    db: &Db,
    name: &str,
    description: &str,
    system_prompt: &str,
) -> Result<Transform> {
    let n = name.trim();
    let p = system_prompt.trim();
    if n.is_empty() {
        return Err(anyhow::anyhow!("name is required"));
    }
    if p.is_empty() {
        return Err(anyhow::anyhow!("system prompt is required"));
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let conn = db.lock();
    let next_order: i64 = conn
        .query_row("SELECT COALESCE(MAX(sort_order), -1) + 1 FROM transforms", [], |r| r.get(0))
        .unwrap_or(0);
    conn.execute(
        "INSERT INTO transforms
            (name, description, system_prompt, is_default, sort_order, hit_count, created_at)
         VALUES (?, ?, ?, 0, ?, 0, ?)",
        params![n, description.trim(), p, next_order, now],
    )?;
    let id = conn.last_insert_rowid();
    Ok(Transform {
        id,
        name: n.to_string(),
        description: description.trim().to_string(),
        system_prompt: p.to_string(),
        is_default: false,
        sort_order: next_order,
        hit_count: 0,
        created_at: now,
    })
}

pub fn update_transform(
    db: &Db,
    id: i64,
    name: &str,
    description: &str,
    system_prompt: &str,
) -> Result<()> {
    let n = name.trim();
    let p = system_prompt.trim();
    if n.is_empty() || p.is_empty() {
        return Err(anyhow::anyhow!("name and prompt are required"));
    }
    let conn = db.lock();
    let affected = conn.execute(
        "UPDATE transforms SET name = ?, description = ?, system_prompt = ? WHERE id = ?",
        params![n, description.trim(), p, id],
    )?;
    if affected == 0 {
        return Err(anyhow::anyhow!("no transform with id {id}"));
    }
    Ok(())
}

pub fn delete_transform(db: &Db, id: i64) -> Result<()> {
    let conn = db.lock();
    let was_default: i64 = conn
        .query_row("SELECT is_default FROM transforms WHERE id = ?", [id], |r| r.get(0))
        .unwrap_or(0);
    let affected = conn.execute("DELETE FROM transforms WHERE id = ?", params![id])?;
    if affected == 0 {
        return Err(anyhow::anyhow!("no transform with id {id}"));
    }
    // If we deleted the default, promote the first remaining transform.
    if was_default == 1 {
        let _ = conn.execute(
            "UPDATE transforms SET is_default = 1
             WHERE id = (SELECT id FROM transforms ORDER BY sort_order ASC LIMIT 1)",
            [],
        );
    }
    Ok(())
}

pub fn set_default_transform(db: &Db, id: i64) -> Result<()> {
    let conn = db.lock();
    conn.execute("UPDATE transforms SET is_default = 0", [])?;
    let affected = conn.execute(
        "UPDATE transforms SET is_default = 1 WHERE id = ?",
        params![id],
    )?;
    if affected == 0 {
        return Err(anyhow::anyhow!("no transform with id {id}"));
    }
    Ok(())
}

pub fn reset_transforms_to_defaults(db: &Db) -> Result<()> {
    let conn = db.lock();
    conn.execute("DELETE FROM transforms", [])?;
    drop(conn);
    let conn = db.lock();
    seed_default_transforms(&conn)?;
    Ok(())
}

#[derive(Debug, Serialize, serde::Deserialize, Clone)]
pub struct Note {
    pub id: i64,
    pub title: String,
    pub body: String,
    pub created_at: i64,
    pub updated_at: i64,
}

pub fn list_notes(db: &Db) -> Result<Vec<Note>> {
    let conn = db.lock();
    let mut stmt = conn.prepare(
        "SELECT id, title, body, created_at, updated_at
         FROM notes ORDER BY updated_at DESC",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok(Note {
                id: r.get(0)?,
                title: r.get(1)?,
                body: r.get(2)?,
                created_at: r.get(3)?,
                updated_at: r.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn get_note(db: &Db, id: i64) -> Result<Note> {
    let conn = db.lock();
    let row = conn.query_row(
        "SELECT id, title, body, created_at, updated_at FROM notes WHERE id = ?",
        [id],
        |r| {
            Ok(Note {
                id: r.get(0)?,
                title: r.get(1)?,
                body: r.get(2)?,
                created_at: r.get(3)?,
                updated_at: r.get(4)?,
            })
        },
    )?;
    Ok(row)
}

pub fn create_note(db: &Db, title: &str, body: &str) -> Result<Note> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let conn = db.lock();
    conn.execute(
        "INSERT INTO notes (title, body, created_at, updated_at) VALUES (?, ?, ?, ?)",
        params![title, body, now, now],
    )?;
    let id = conn.last_insert_rowid();
    Ok(Note {
        id,
        title: title.to_string(),
        body: body.to_string(),
        created_at: now,
        updated_at: now,
    })
}

pub fn update_note(db: &Db, id: i64, title: &str, body: &str) -> Result<()> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let conn = db.lock();
    let affected = conn.execute(
        "UPDATE notes SET title = ?, body = ?, updated_at = ? WHERE id = ?",
        params![title, body, now, id],
    )?;
    if affected == 0 {
        return Err(anyhow::anyhow!("no note with id {id}"));
    }
    Ok(())
}

pub fn delete_note(db: &Db, id: i64) -> Result<()> {
    let conn = db.lock();
    let affected = conn.execute("DELETE FROM notes WHERE id = ?", params![id])?;
    if affected == 0 {
        return Err(anyhow::anyhow!("no note with id {id}"));
    }
    Ok(())
}

pub fn bump_transform_hits(db: &Db, id: i64) -> Result<()> {
    let conn = db.lock();
    conn.execute(
        "UPDATE transforms SET hit_count = hit_count + 1 WHERE id = ?",
        params![id],
    )?;
    Ok(())
}

pub fn bump_dictionary_hits(db: &Db, hits: &[(i64, i64)]) -> Result<()> {
    if hits.is_empty() {
        return Ok(());
    }
    let conn = db.lock();
    for (id, delta) in hits {
        conn.execute(
            "UPDATE dictionary SET hit_count = hit_count + ? WHERE id = ?",
            params![*delta, *id],
        )?;
    }
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct AppCategoryUsage {
    pub category: String,
    pub count: i64,
    pub percentage: f32,
}

#[derive(Debug, Serialize)]
pub struct HeatmapDay {
    pub date: String,
    pub count: i64,
}

#[derive(Debug, Serialize)]
pub struct UsageStats {
    pub wpm: f32,
    pub wpm_percentile: f32,
    pub total_words: i64,
    pub words_this_month: i64,
    pub words_last_month: i64,
    pub mom_change_pct: Option<f32>,
    pub total_fixes: i64,
    pub ai_fixes: i64,
    pub dictionary_fixes: i64,
    pub day_streak: i64,
    pub longest_streak: i64,
    pub total_apps_used: i64,
    pub app_usage: Vec<AppCategoryUsage>,
    pub heatmap: Vec<HeatmapDay>,
}

pub fn usage_stats(db: &Db) -> Result<UsageStats> {
    let conn = db.lock();

    let total_words: i64 = conn
        .query_row("SELECT COALESCE(SUM(word_count), 0) FROM dictations", [], |r| r.get(0))
        .unwrap_or(0);
    let total_fixes: i64 = conn
        .query_row("SELECT COALESCE(SUM(fix_count), 0) FROM dictations", [], |r| r.get(0))
        .unwrap_or(0);
    let dictionary_fixes: i64 = conn
        .query_row("SELECT COALESCE(SUM(hit_count), 0) FROM dictionary", [], |r| r.get(0))
        .unwrap_or(0);
    let ai_fixes = (total_fixes - dictionary_fixes).max(0);

    // WPM over all dictation duration.
    let (words_all, ms_all): (i64, i64) = conn
        .query_row(
            "SELECT COALESCE(SUM(word_count), 0), COALESCE(SUM(duration_ms), 0) FROM dictations",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap_or((0, 0));
    let wpm = if ms_all > 0 {
        (words_all as f64 / (ms_all as f64 / 60_000.0)) as f32
    } else {
        0.0
    };

    // Month-over-month words.
    let words_this_month: i64 = conn
        .query_row(
            "SELECT COALESCE(SUM(word_count), 0) FROM dictations
             WHERE date(ts, 'unixepoch') >= date('now', 'start of month')",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let words_last_month: i64 = conn
        .query_row(
            "SELECT COALESCE(SUM(word_count), 0) FROM dictations
             WHERE date(ts, 'unixepoch') >= date('now', 'start of month', '-1 month')
               AND date(ts, 'unixepoch') <  date('now', 'start of month')",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let mom_change_pct = if words_last_month > 0 {
        Some((words_this_month - words_last_month) as f32 / words_last_month as f32 * 100.0)
    } else if words_this_month > 0 {
        None
    } else {
        Some(0.0)
    };

    let day_streak = compute_streak(&conn);
    let longest_streak = compute_longest_streak(&conn);

    // Distinct apps with at least one dictation.
    let total_apps_used: i64 = conn
        .query_row(
            "SELECT COUNT(DISTINCT foreground_app) FROM dictations WHERE foreground_app IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    // Per-category dictation counts.
    let app_usage = compute_app_usage(&conn);

    // Heatmap: last 90 days by date.
    let heatmap = compute_heatmap(&conn, 90);

    Ok(UsageStats {
        wpm,
        wpm_percentile: wpm_percentile_estimate(wpm),
        total_words,
        words_this_month,
        words_last_month,
        mom_change_pct,
        total_fixes,
        ai_fixes,
        dictionary_fixes,
        day_streak,
        longest_streak,
        total_apps_used,
        app_usage,
        heatmap,
    })
}

#[derive(Debug, Serialize)]
pub struct VoiceStats {
    pub most_used_word: Option<String>,
    pub most_corrected_word: Option<String>,
    pub catchphrase: Option<String>,
    pub peak_day_name: Option<String>,
    pub peak_hour_label: Option<String>,
    pub peak_app: Option<String>,
    pub peak_app_category: Option<String>,
    pub voice_narrative: Option<String>,
    pub peak_narrative: Option<String>,
    pub last_generated_at: Option<i64>,
    pub words_since_last_gen: i64,
    pub min_words_to_refresh: i64,
    pub total_words: i64,
    pub has_api_key: bool,
}

const MIN_WORDS_BEFORE_REFRESH: i64 = 200;

pub fn voice_stats(db: &Db, has_api_key: bool) -> Result<VoiceStats> {
    let conn = db.lock();

    let total_words: i64 = conn
        .query_row("SELECT COALESCE(SUM(word_count), 0) FROM dictations", [], |r| r.get(0))
        .unwrap_or(0);

    let texts = pull_cleaned_texts(&conn, 500);
    let raw_pairs = pull_text_pairs(&conn, 500);

    let most_used_word = top_meaningful_word(&texts);
    let most_corrected_word = top_corrected_word(&raw_pairs);
    let catchphrase = top_catchphrase(&texts);

    let peak = conn
        .query_row(
            "SELECT
                strftime('%w', ts, 'unixepoch', 'localtime') AS dow,
                strftime('%H', ts, 'unixepoch', 'localtime') AS hour,
                COUNT(*) AS n
             FROM dictations
             GROUP BY dow, hour
             HAVING n > 0
             ORDER BY n DESC
             LIMIT 1",
            [],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )
        .ok();

    let (peak_day_name, peak_hour_label, peak_app, peak_app_category) = match peak {
        Some((dow, hour)) => {
            let day = day_name(&dow).to_string();
            let hour_label = format_hour(&hour);
            let app = conn
                .query_row(
                    "SELECT foreground_app FROM dictations
                     WHERE strftime('%w', ts, 'unixepoch', 'localtime') = ?
                       AND strftime('%H', ts, 'unixepoch', 'localtime') = ?
                       AND foreground_app IS NOT NULL
                     GROUP BY foreground_app
                     ORDER BY COUNT(*) DESC
                     LIMIT 1",
                    [&dow, &hour],
                    |r| r.get::<_, String>(0),
                )
                .ok();
            let category = app.as_deref().map(|a| categorize_app(a).to_string());
            (Some(day), Some(hour_label), app, category)
        }
        None => (None, None, None, None),
    };

    let (voice_narrative, peak_narrative, last_generated_at, last_word_count): (
        Option<String>,
        Option<String>,
        Option<i64>,
        i64,
    ) = conn
        .query_row(
            "SELECT voice_narrative, peak_narrative, last_generated_at, last_word_count
             FROM voice_profile WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap_or((None, None, None, 0));

    let words_since_last_gen = (total_words - last_word_count).max(0);

    Ok(VoiceStats {
        most_used_word,
        most_corrected_word,
        catchphrase,
        peak_day_name,
        peak_hour_label,
        peak_app,
        peak_app_category,
        voice_narrative,
        peak_narrative,
        last_generated_at,
        words_since_last_gen,
        min_words_to_refresh: MIN_WORDS_BEFORE_REFRESH,
        total_words,
        has_api_key,
    })
}

/// Cheap read of just the last voice-profile generation timestamp. Used on
/// the dictation hot path to decide whether an auto-refresh is due, without
/// running the full (expensive) `voice_stats` aggregation every time.
pub fn voice_last_generated_at(db: &Db) -> Result<Option<i64>> {
    let conn = db.lock();
    let ts: Option<i64> = conn.query_row(
        "SELECT last_generated_at FROM voice_profile WHERE id = 1",
        [],
        |r| r.get(0),
    )?;
    Ok(ts)
}

pub fn save_voice_narrative(
    db: &Db,
    voice_narrative: &str,
    peak_narrative: &str,
) -> Result<()> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let conn = db.lock();
    let total_words: i64 = conn
        .query_row("SELECT COALESCE(SUM(word_count), 0) FROM dictations", [], |r| r.get(0))
        .unwrap_or(0);
    conn.execute(
        "UPDATE voice_profile SET
            voice_narrative = ?,
            peak_narrative = ?,
            last_generated_at = ?,
            last_word_count = ?
         WHERE id = 1",
        params![voice_narrative, peak_narrative, now, total_words],
    )?;
    Ok(())
}

pub fn voice_profile_context(db: &Db) -> Result<String> {
    let conn = db.lock();
    let texts = pull_cleaned_texts(&conn, 30);
    let stats_summary = format!(
        "Recent dictation samples (most recent first, one per line):\n{}",
        texts
            .iter()
            .take(30)
            .map(|t| format!("- {}", t.chars().take(280).collect::<String>()))
            .collect::<Vec<_>>()
            .join("\n")
    );
    Ok(stats_summary)
}

fn pull_cleaned_texts(conn: &Connection, limit: i64) -> Vec<String> {
    let mut stmt = match conn.prepare(
        "SELECT cleaned_text FROM dictations ORDER BY ts DESC LIMIT ?",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    stmt.query_map([limit], |r| r.get::<_, String>(0))
        .ok()
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
}

fn pull_text_pairs(conn: &Connection, limit: i64) -> Vec<(String, String)> {
    let mut stmt = match conn.prepare(
        "SELECT raw_text, cleaned_text FROM dictations ORDER BY ts DESC LIMIT ?",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    stmt.query_map([limit], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .ok()
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
}

fn tokenize_words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric() && c != '\'')
        .filter(|w| !w.is_empty())
        .map(|w| w.to_lowercase())
        .collect()
}

fn is_meaningful(word: &str) -> bool {
    word.len() > 2 && !STOP_WORDS.contains(&word)
}

fn top_meaningful_word(texts: &[String]) -> Option<String> {
    let mut counts: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    for text in texts {
        for tok in tokenize_words(text) {
            if is_meaningful(&tok) {
                *counts.entry(tok).or_insert(0) += 1;
            }
        }
    }
    counts.into_iter().max_by_key(|(_, n)| *n).map(|(w, _)| w)
}

fn top_corrected_word(pairs: &[(String, String)]) -> Option<String> {
    let mut counts: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    for (raw, cleaned) in pairs {
        let raw_set: HashSet<String> = tokenize_words(raw).into_iter().collect();
        let cleaned_set: HashSet<String> = tokenize_words(cleaned).into_iter().collect();
        for token in raw_set.difference(&cleaned_set) {
            if is_meaningful(token) {
                *counts.entry(token.clone()).or_insert(0) += 1;
            }
        }
    }
    counts.into_iter().max_by_key(|(_, n)| *n).map(|(w, _)| w)
}

fn top_catchphrase(texts: &[String]) -> Option<String> {
    let mut ngrams: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    for text in texts {
        let words: Vec<String> = text
            .split_whitespace()
            .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'').to_lowercase())
            .filter(|w| !w.is_empty())
            .collect();
        for n in 3..=5 {
            if words.len() < n {
                continue;
            }
            for window in words.windows(n) {
                let all_stop = window.iter().all(|w| STOP_WORDS.contains(&w.as_str()));
                if all_stop {
                    continue;
                }
                let phrase = window.join(" ");
                if phrase.len() < 12 {
                    continue;
                }
                *ngrams.entry(phrase).or_insert(0) += 1;
            }
        }
    }
    ngrams
        .into_iter()
        .filter(|(_, n)| *n >= 2)
        .max_by_key(|(p, n)| (*n, p.len() as i64))
        .map(|(p, _)| p)
}

fn day_name(dow: &str) -> &'static str {
    match dow {
        "0" => "Sunday",
        "1" => "Monday",
        "2" => "Tuesday",
        "3" => "Wednesday",
        "4" => "Thursday",
        "5" => "Friday",
        "6" => "Saturday",
        _ => "—",
    }
}

fn format_hour(hour: &str) -> String {
    let h: u32 = hour.parse().unwrap_or(0);
    let (display, suffix) = match h {
        0 => (12, "a.m."),
        1..=11 => (h, "a.m."),
        12 => (12, "p.m."),
        _ => (h - 12, "p.m."),
    };
    format!("{} {}", display, suffix)
}

/// Best-guess percentile mapping (we don't have a real population). Keeps the
/// gauge value monotonic with WPM so users see it move with practice.
fn wpm_percentile_estimate(wpm: f32) -> f32 {
    if wpm >= 200.0 { 0.1 }
    else if wpm >= 150.0 { 1.0 }
    else if wpm >= 120.0 { 5.0 }
    else if wpm >= 100.0 { 20.0 }
    else if wpm >= 80.0 { 50.0 }
    else if wpm >= 60.0 { 75.0 }
    else { 90.0 }
}

fn compute_longest_streak(conn: &Connection) -> i64 {
    let mut stmt = match conn.prepare(
        "SELECT DISTINCT date(ts, 'unixepoch', 'localtime') AS d FROM dictations ORDER BY d ASC",
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
    let mut longest = 1i64;
    let mut current = 1i64;
    for pair in days.windows(2) {
        let next: String = match conn.query_row(
            "SELECT date(?, '+1 day')",
            [&pair[0]],
            |r| r.get::<_, String>(0),
        ) {
            Ok(d) => d,
            Err(_) => break,
        };
        if pair[1] == next {
            current += 1;
            if current > longest { longest = current; }
        } else {
            current = 1;
        }
    }
    longest
}

fn compute_app_usage(conn: &Connection) -> Vec<AppCategoryUsage> {
    let mut stmt = match conn.prepare(
        "SELECT foreground_app, COUNT(*) FROM dictations
         WHERE foreground_app IS NOT NULL
         GROUP BY foreground_app",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows: Vec<(String, i64)> = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
        .ok()
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default();

    let mut buckets: std::collections::HashMap<&'static str, i64> = std::collections::HashMap::new();
    for (app, count) in &rows {
        *buckets.entry(categorize_app(app)).or_insert(0) += *count;
    }
    let total: i64 = buckets.values().sum();
    let mut out: Vec<AppCategoryUsage> = buckets
        .into_iter()
        .map(|(category, count)| AppCategoryUsage {
            category: category.to_string(),
            count,
            percentage: if total > 0 { count as f32 / total as f32 * 100.0 } else { 0.0 },
        })
        .collect();
    out.sort_by(|a, b| b.count.cmp(&a.count));
    out
}

fn categorize_app(exe: &str) -> &'static str {
    let lower = exe.to_lowercase();
    let stem = lower.trim_end_matches(".exe");
    match stem {
        // AI assistants
        "chatgpt" | "claude" | "perplexity" | "gemini" | "copilot" => "AI Prompts",
        // Code editors
        "code" | "code - insiders" | "cursor" | "windsurf"
        | "rider" | "idea64" | "pycharm64" | "goland64" | "webstorm64" | "rustrover64"
        | "sublime_text" | "atom" | "nvim-qt" => "Code",
        // Office / writing
        "winword" | "powerpnt" | "excel" | "onenote" | "notion" | "obsidian" | "typora"
        | "scrivener" => "Documents",
        // Mail
        "outlook" | "thunderbird" | "hostedgmaildesktopapp" => "Emails",
        // Work messaging
        "slack" | "teams" | "discord" => "Work messages",
        // Personal messaging
        "whatsapp" | "telegram" | "signal" | "messenger" => "Personal messages",
        // Browsers
        "chrome" | "msedge" | "firefox" | "brave" | "opera" | "arc" | "vivaldi" => "Browsing",
        // Shells / system
        "windowsterminal" | "wt" | "powershell" | "pwsh" | "cmd" | "explorer" => "System",
        _ => "Other",
    }
}

fn compute_heatmap(conn: &Connection, days: i64) -> Vec<HeatmapDay> {
    let mut stmt = match conn.prepare(
        "SELECT date(ts, 'unixepoch', 'localtime') AS d, COUNT(*) AS n
         FROM dictations
         WHERE date(ts, 'unixepoch', 'localtime') >= date('now', 'localtime', ?)
         GROUP BY d ORDER BY d ASC",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let offset = format!("-{} days", days);
    stmt.query_map([offset], |r| {
        Ok(HeatmapDay {
            date: r.get(0)?,
            count: r.get(1)?,
        })
    })
    .ok()
    .map(|rows| rows.filter_map(|r| r.ok()).collect())
    .unwrap_or_default()
}

fn compute_streak(conn: &Connection) -> i64 {
    // All date computations use 'localtime' so the user's "today" matches
    // what the dashboard renders client-side.
    let mut stmt = match conn.prepare(
        "SELECT DISTINCT date(ts, 'unixepoch', 'localtime') AS d
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

    let today: String = match conn.query_row(
        "SELECT date('now', 'localtime')",
        [],
        |r| r.get::<_, String>(0),
    ) {
        Ok(d) => d,
        Err(_) => return 0,
    };

    let yesterday: String = match conn.query_row(
        "SELECT date('now', 'localtime', '-1 day')",
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
