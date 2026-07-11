//! Opt-in anonymous telemetry, posted directly to a Supabase REST endpoint.
//!
//! Design constraints:
//! - **Opt-in only.** Caller checks `cfg.telemetry_enabled` before calling
//!   `track()`. We don't read config here so the gate is one line at every
//!   call site (and there's no implicit "fire and forget" path that bypasses
//!   the user's choice).
//! - **No content, ever.** Allowed: counts, durations, modes, languages,
//!   error categories, app version, OS family, anonymous UUID.
//!   Forbidden: transcripts, audio bytes, API keys, dictionary entries,
//!   notes, foreground exe names (those leak app-usage habits).
//! - **Never blocks the user.** All sends are spawned onto the async
//!   runtime; failures are logged at debug and dropped. Telemetry must
//!   never delay a dictation or surface an error to the UI.
//! - **Anon ID is durable and local.** A v4 UUID is generated on first run
//!   and persisted to `%APPDATA%\Bulbul\telemetry_id`. Resetting the file
//!   gives the user a fresh identity.
//! - **The publishable key in the binary is safe by design.** The Supabase
//!   `events` table has Row-Level Security enabled with an INSERT-only
//!   policy for the `anon` role; an attacker who extracts the key can only
//!   add rows, never read or modify others'.

use crate::config;
use parking_lot::Mutex;
use serde::Serialize;
use serde_json::Value;
use std::fs;
use std::sync::OnceLock;
use std::time::Duration;
use uuid::Uuid;

/// Supabase project URL. Hardcoded — this is public infrastructure, not a
/// secret. The endpoint is protected by RLS, not by hiding the URL.
const PROJECT_URL: &str = "https://mpzuaarkdhdykpbkgyrs.supabase.co";

/// Supabase publishable (anon) key. Safe to ship in clients: the project's
/// `events` table has INSERT-only RLS for the `anon` role, so this key
/// cannot read or modify existing data.
const PUBLISHABLE_KEY: &str = "sb_publishable_rGLwve2RSBeHG7mZRXYgSw_YUeJcF0x";

/// Maximum events held in the in-memory buffer between flushes. Hit this
/// and we flush immediately. Set low because each event is small and we'd
/// rather burn one extra HTTP request than risk losing many events on
/// crash.
const FLUSH_AT_SIZE: usize = 10;

/// Hard cap on buffered events. If the network is down and we keep
/// accumulating, drop the OLDEST events past this so memory stays bounded.
/// We deliberately keep newest events (latest dictations, recent errors
/// are more actionable than old ones).
const BUFFER_CAP: usize = 100;

/// Periodic flush cadence. Even at low event rates, this keeps the
/// dashboard reasonably fresh.
const FLUSH_PERIOD: Duration = Duration::from_secs(30);

/// Per-request timeout. Telemetry is best-effort — if Supabase is slow,
/// the request gives up rather than holding memory.
const POST_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Serialize, Clone)]
struct Event {
    anon_id: String,
    app_version: String,
    os: String,
    event_name: String,
    props: Value,
}

static ANON_ID: OnceLock<String> = OnceLock::new();
static BUFFER: OnceLock<Mutex<Vec<Event>>> = OnceLock::new();
static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

fn buffer() -> &'static Mutex<Vec<Event>> {
    BUFFER.get_or_init(|| Mutex::new(Vec::new()))
}

fn client() -> &'static reqwest::Client {
    CLIENT.get_or_init(reqwest::Client::new)
}

/// Read the anonymous ID from `telemetry_id`, generating it if absent.
/// Cached for the rest of the process via `OnceLock` so we touch disk once
/// per session.
fn anon_id() -> &'static String {
    ANON_ID.get_or_init(|| {
        let path = match config::config_dir() {
            Ok(d) => d.join("telemetry_id"),
            Err(e) => {
                tracing::debug!("telemetry: could not resolve config dir: {e:#}");
                return Uuid::new_v4().to_string();
            }
        };
        if let Ok(s) = fs::read_to_string(&path) {
            let trimmed = s.trim().to_string();
            if Uuid::parse_str(&trimmed).is_ok() {
                return trimmed;
            }
        }
        let id = Uuid::new_v4().to_string();
        if let Err(e) = fs::write(&path, &id) {
            tracing::debug!("telemetry: could not persist anon_id: {e:#}");
        }
        id
    })
}

/// Coarse OS bucket. Deliberately omits build number, locale, hostname.
/// "windows" is fine for v0 — we can split Win10/Win11 later if we ever
/// need to gate features by OS version.
fn os_label() -> String {
    if cfg!(target_os = "windows") {
        "windows".to_string()
    } else if cfg!(target_os = "macos") {
        "macos".to_string()
    } else if cfg!(target_os = "linux") {
        "linux".to_string()
    } else {
        "other".to_string()
    }
}

/// Enqueue an event. Caller is responsible for gating on the user's
/// `telemetry_enabled` setting — this function does not consult Config.
///
/// Buffered in memory and flushed either when the buffer fills or on the
/// periodic timer (see `spawn_periodic_flush`).
pub fn track(event_name: &str, props: Value) {
    let evt = Event {
        anon_id: anon_id().clone(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        os: os_label(),
        event_name: event_name.to_string(),
        props,
    };
    let should_flush = {
        let mut b = buffer().lock();
        b.push(evt);
        if b.len() > BUFFER_CAP {
            // Drop oldest to stay bounded. Latest events are more actionable.
            let excess = b.len() - BUFFER_CAP;
            b.drain(0..excess);
        }
        b.len() >= FLUSH_AT_SIZE
    };
    if should_flush {
        spawn_flush();
    }
}

/// Start a background loop that flushes the buffer every `FLUSH_PERIOD`.
/// Call once during app setup.
pub fn spawn_periodic_flush() {
    tauri::async_runtime::spawn(async {
        loop {
            tokio::time::sleep(FLUSH_PERIOD).await;
            spawn_flush();
        }
    });
}

fn spawn_flush() {
    let drained: Vec<Event> = {
        let mut b = buffer().lock();
        if b.is_empty() {
            return;
        }
        b.drain(..).collect()
    };
    tauri::async_runtime::spawn(async move {
        if let Err(e) = post(&drained).await {
            tracing::debug!("telemetry post failed ({} events): {e:#}", drained.len());
            // Drop on failure. A side-project telemetry stream that retries
            // forever ends up burning battery for no benefit.
        }
    });
}

async fn post(events: &[Event]) -> anyhow::Result<()> {
    let url = format!("{PROJECT_URL}/rest/v1/events");
    let resp = client()
        .post(&url)
        .header("apikey", PUBLISHABLE_KEY)
        .header("Authorization", format!("Bearer {PUBLISHABLE_KEY}"))
        .header("Content-Type", "application/json")
        .header("Prefer", "return=minimal")
        .json(events)
        .timeout(POST_TIMEOUT)
        .send()
        .await?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("telemetry POST {status}: {body}");
    }
    Ok(())
}

/// Bucket a duration so we never accidentally fingerprint a user by an
/// unusually-precise dictation length.
pub fn duration_bucket(ms: u64) -> &'static str {
    match ms {
        0..=1999 => "<2s",
        2000..=4999 => "2-5s",
        5000..=9999 => "5-10s",
        10000..=29999 => "10-30s",
        _ => "30s+",
    }
}

/// Bucket a word count for the same reason.
pub fn word_count_bucket(n: usize) -> &'static str {
    match n {
        0..=5 => "1-5",
        6..=20 => "6-20",
        21..=50 => "21-50",
        51..=100 => "51-100",
        _ => "100+",
    }
}
