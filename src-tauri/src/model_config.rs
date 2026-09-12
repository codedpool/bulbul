// SPDX-License-Identifier: GPL-3.0-only
// Copyright (c) 2026 Romanch Roshan Singh

//! Remote-hosted cleanup-model chain, fetched from bulbultypes.xyz so a
//! Groq model rotation (like qwen3.6-27b's 2026-09-14 retirement) can be
//! fixed by editing a JSON file and redeploying the site — no app release,
//! no store review. Fetched periodically in the background and cached to
//! a local file; `groq::cleanup_chain` only ever reads that local cache —
//! the network fetch never sits in the record→transcribe→cleanup→inject
//! path. If nothing has ever fetched successfully (offline first run, or
//! the site is unreachable), callers fall back to their own embedded seed
//! chain — this module reports that as `None`, not an error.

use crate::config::config_dir;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::PathBuf;
use std::time::Duration;
use tauri::AppHandle;

const MODELS_URL: &str = "https://bulbultypes.xyz/models.json";
/// Matches the existing GitHub update-watcher's cadence — one shared
/// "how often do we phone home for something non-urgent" rhythm.
const POLL_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Deserialize)]
struct RemoteModelConfig {
    cleanup_chain: Vec<String>,
}

fn cache_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("model_chain.json"))
}

/// Reads the cached remote cleanup chain, if a fetch has ever succeeded.
/// A local disk read only — no network — so callers on the dictation hot
/// path never block on this. `None` means "nothing cached yet, use your
/// own embedded default" — not an error.
pub fn cached_cleanup_chain() -> Option<Vec<String>> {
    let path = cache_path().ok()?;
    let text = std::fs::read_to_string(path).ok()?;
    let parsed: RemoteModelConfig = serde_json::from_str(&text).ok()?;
    if parsed.cleanup_chain.is_empty() {
        None
    } else {
        Some(parsed.cleanup_chain)
    }
}

async fn fetch_and_cache_once() -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .context("building http client")?;
    let text = client
        .get(MODELS_URL)
        .send()
        .await
        .context("GET models.json")?
        .text()
        .await
        .context("reading models.json body")?;
    // Validate before writing so a malformed or empty remote file can't
    // clobber a previously-good cache with garbage.
    let parsed: RemoteModelConfig =
        serde_json::from_str(&text).context("parsing models.json")?;
    if parsed.cleanup_chain.is_empty() {
        anyhow::bail!("models.json cleanup_chain is empty — ignoring");
    }
    std::fs::write(cache_path()?, &text).context("writing model_chain cache")?;
    Ok(())
}

/// Background poller, same shape as `spawn_update_watcher`: a grace period
/// after boot, then a steady cadence, forever. Failures are logged and
/// swallowed — the cache just keeps whatever it last had (or the caller's
/// embedded seed chain, if nothing has ever succeeded yet).
pub fn spawn_model_config_watcher(_app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(10)).await;
        loop {
            if let Err(e) = fetch_and_cache_once().await {
                tracing::warn!("model config fetch failed: {e:#}");
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    });
}
