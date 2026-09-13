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
//!
//! Safety net: a fetched chain is cross-checked against Groq's own live
//! `/v1/models` list (using whatever Groq key the user has configured)
//! before being cached, so a stale or forgotten models.json update can't
//! lead dictation with a model Groq has since retired. This is strictly
//! best-effort — no key yet, an offline check, or every model in the chain
//! turning up missing all fall back to caching the chain exactly as
//! fetched, the same as before this existed. It never blocks or degrades
//! the base fetch-and-cache behavior, only tightens it when it can.

use crate::config::config_dir;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;
use tauri::{AppHandle, Manager};

const MODELS_URL: &str = "https://bulbultypes.xyz/models.json";
/// Matches the existing GitHub update-watcher's cadence — one shared
/// "how often do we phone home for something non-urgent" rhythm.
const POLL_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Deserialize, Serialize)]
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

/// Cross-checks a fetched chain against Groq's live model list, filtering
/// out anything Groq no longer serves. Best-effort by design: returns the
/// chain unchanged if there's no key to check with, the live check itself
/// fails (network, bad key), or filtering would leave nothing at all —
/// none of those should ever make the chain worse than not checking.
async fn verify_against_groq(chain: Vec<String>, api_key: &str) -> Vec<String> {
    if api_key.trim().is_empty() {
        return chain;
    }
    let live = match crate::groq::list_groq_models(api_key).await {
        Ok(models) => models,
        Err(e) => {
            tracing::warn!("Groq live-model cross-check failed, caching chain as fetched: {e:#}");
            return chain;
        }
    };
    let verified: Vec<String> = chain
        .iter()
        .filter(|m| live.iter().any(|l| l == *m))
        .cloned()
        .collect();
    if verified.is_empty() {
        tracing::warn!(
            "none of the fetched cleanup_chain models are in Groq's live list ({:?}); \
             keeping the chain as fetched rather than caching an empty one",
            chain
        );
        chain
    } else {
        if verified.len() != chain.len() {
            tracing::info!(
                "cleanup_chain trimmed against Groq's live models: {chain:?} -> {verified:?}"
            );
        }
        verified
    }
}

async fn fetch_and_cache_once(api_key: &str) -> Result<()> {
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
    let mut parsed: RemoteModelConfig =
        serde_json::from_str(&text).context("parsing models.json")?;
    if parsed.cleanup_chain.is_empty() {
        anyhow::bail!("models.json cleanup_chain is empty — ignoring");
    }
    parsed.cleanup_chain = verify_against_groq(parsed.cleanup_chain, api_key).await;
    let text = serde_json::to_string(&parsed).context("serializing verified model chain")?;
    std::fs::write(cache_path()?, &text).context("writing model_chain cache")?;
    Ok(())
}

/// Background poller, same shape as `spawn_update_watcher`: a grace period
/// after boot, then a steady cadence, forever. Failures are logged and
/// swallowed — the cache just keeps whatever it last had (or the caller's
/// embedded seed chain, if nothing has ever succeeded yet).
pub fn spawn_model_config_watcher(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(10)).await;
        loop {
            let api_key = app
                .state::<crate::AppState>()
                .config
                .lock()
                .groq_api_key
                .clone();
            if let Err(e) = fetch_and_cache_once(&api_key).await {
                tracing::warn!("model config fetch failed: {e:#}");
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    });
}
