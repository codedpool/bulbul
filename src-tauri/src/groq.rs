// SPDX-License-Identifier: GPL-3.0-only
// Copyright (c) 2026 Romanch Roshan Singh

use crate::config::{CleanupMode, Config};
use anyhow::{anyhow, Context, Result};
use reqwest::multipart;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use std::time::Duration;

const BASE_URL: &str = "https://api.groq.com/openai/v1";

/// Cerebras Cloud — OpenAI-compatible. Used only for the cleanup /
/// text-conversion stage when the user has added a Cerebras key (see
/// [chat_endpoint]); Whisper STT always stays on Groq, which is why the Groq
/// key is still required even with Cerebras configured.
const CEREBRAS_BASE_URL: &str = "https://api.cerebras.ai/v1";

/// Fallback Cerebras model when a Cerebras key is set but no model has been
/// picked yet. gpt-oss is fast on Cerebras and keeps its reasoning out of the
/// message content, so it won't trip the cleanup expansion guard.
pub const DEFAULT_CEREBRAS_MODEL: &str = "gpt-oss-120b";

/// One reqwest Client for the whole app lifetime — reusing TCP+TLS sessions
/// across STT, cleanup, transform and validate calls. Doesn't change what we
/// send to Groq (request count, prompt size, token billing are all identical
/// — Groq accounts per-request, not per-connection). It just skips the
/// handshake bytes on every dictation after the first.
fn shared_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(reqwest::Client::new)
}

/// Hard cap on attempts (1 initial + 3 retries) before giving up on a
/// rate-limited or transiently-failing Groq request.
const MAX_ATTEMPTS: u32 = 4;

/// Callback fired right before each backoff sleep, with the wait in seconds,
/// so the UI can show "retrying in Ns" instead of appearing frozen.
pub type RetryNotify<'a> = dyn Fn(u64) + Send + Sync + 'a;

/// Send a Groq request, retrying on 429 (rate limit) and 5xx with backoff,
/// and return the response body on success. Honors the `Retry-After` header
/// when present, otherwise uses exponential backoff capped at 30s. `make` is
/// invoked fresh for every attempt because request bodies (multipart forms,
/// JSON) can't be reused across sends.
async fn send_with_retry(
    make: impl Fn() -> reqwest::RequestBuilder,
    label: &str,
    notify: Option<&RetryNotify<'_>>,
) -> Result<String> {
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        let resp = make()
            .send()
            .await
            .with_context(|| format!("POST {label}"))?;
        let status = resp.status();
        if (status.as_u16() == 429 || status.is_server_error()) && attempt < MAX_ATTEMPTS {
            let wait = retry_wait_secs(&resp, attempt);
            tracing::warn!("{label}: {status}; retry {attempt}/{MAX_ATTEMPTS} after {wait}s");
            if let Some(n) = notify {
                n(wait);
            }
            tokio::time::sleep(Duration::from_secs(wait)).await;
            continue;
        }
        let body = resp
            .text()
            .await
            .with_context(|| format!("reading {label} response body"))?;
        if status.as_u16() == 429 {
            return Err(anyhow!(
                "The API is rate-limited right now. Wait a few seconds and try again."
            ));
        }
        if !status.is_success() {
            return Err(anyhow!("{label} request failed ({status}): {body}"));
        }
        return Ok(body);
    }
}

/// How long to wait before the next attempt: the server's `Retry-After`
/// (seconds) if it sent one, else exponential backoff (2s, 4s, 8s…) capped
/// at 30s.
fn retry_wait_secs(resp: &reqwest::Response, attempt: u32) -> u64 {
    if let Some(secs) = resp
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
    {
        return secs.clamp(1, 30);
    }
    backoff_secs(attempt)
}

/// Exponential backoff: attempt 1 → 2s, 2 → 4s, 3 → 8s, …, capped at 30s.
fn backoff_secs(attempt: u32) -> u64 {
    (1u64 << attempt.min(20)).clamp(2, 30)
}

/// Resolve which provider + model handles a chat/cleanup completion. Whisper
/// STT always stays on Groq (Cerebras has no transcription endpoint), so only
/// the text stages — cleanup, transform, voice profile — consult this. When
/// the user has added a Cerebras key the raw transcript is cleaned on Cerebras
/// with their chosen model; otherwise it stays on Groq with `chat_model`.
/// Returns (base_url, bearer_key, model_id).
pub fn chat_endpoint(cfg: &Config) -> (&'static str, &str, &str) {
    if cfg.has_cerebras_key() {
        let model = cfg.cerebras_model.trim();
        let model = if model.is_empty() {
            DEFAULT_CEREBRAS_MODEL
        } else {
            model
        };
        (CEREBRAS_BASE_URL, cfg.cerebras_api_key.trim(), model)
    } else {
        (BASE_URL, cfg.groq_api_key.trim(), cfg.chat_model.as_str())
    }
}

/// Strip a leading `<think>…</think>` reasoning block that some models (GLM,
/// Qwen3, DeepSeek-R1, …) emit inline before the answer. Left in place, the
/// think trace inflates the word count and trips the cleanup expansion guard,
/// so the user gets the raw transcript instead of the cleanup (witnessed with
/// Qwen3). Groq hides gpt-oss reasoning already; Cerebras' GLM/Qwen surface it
/// in `content`, so we defend here. Returns the trimmed post-think answer.
fn strip_reasoning(text: &str) -> String {
    let t = text.trim_start();
    if let Some(rest) = t.strip_prefix("<think>") {
        return match rest.split_once("</think>") {
            Some((_, after)) => after.trim().to_string(),
            // Unterminated (truncated mid-thought) — nothing usable follows.
            None => String::new(),
        };
    }
    text.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_progression_and_cap() {
        assert_eq!(backoff_secs(1), 2);
        assert_eq!(backoff_secs(2), 4);
        assert_eq!(backoff_secs(3), 8);
        assert_eq!(backoff_secs(4), 16);
        // Never exceeds the 30s ceiling, even for absurd attempt counts.
        assert_eq!(backoff_secs(10), 30);
        assert_eq!(backoff_secs(100), 30);
    }

    #[test]
    fn answer_back_guard_catches_short_answer() {
        // The marquee failure: model treats a question as a task to
        // answer. The reply ("Paris.") is shorter than the raw, so the
        // expansion guard doesn't fire — this guard must.
        let raw = "what's the capital of France";
        let cleaned = "The answer is Paris.";
        assert!(looks_like_answer_back(cleaned, raw));
    }

    #[test]
    fn answer_back_guard_catches_markdown_explanation() {
        let raw = "how do I set up the dev server";
        let cleaned = "Here's how to set up the dev server:\n\n1. Clone the repo\n2. Run npm install";
        assert!(looks_like_answer_back(cleaned, raw));
    }

    #[test]
    fn answer_back_guard_passes_legit_dictation_starting_with_sure() {
        // "Sure, that sounds good" is a real thing people dictate. It
        // starts with a flag-word but overlaps heavily with the raw, so
        // the second condition (low overlap) must save it.
        let raw = "sure that sounds good lets ship it";
        let cleaned = "Sure, that sounds good — let's ship it.";
        assert!(!looks_like_answer_back(cleaned, raw));
    }

    #[test]
    fn answer_back_guard_passes_legit_heres_the_thing() {
        let raw = "heres the thing the rollout is delayed";
        let cleaned = "Here's the thing, the rollout is delayed.";
        assert!(!looks_like_answer_back(cleaned, raw));
    }

    #[test]
    fn answer_back_guard_passes_normal_speech() {
        let raw = "ship the v1 first and iterate later";
        let cleaned = "Ship the v1 first and iterate later.";
        assert!(!looks_like_answer_back(cleaned, raw));
    }
}

#[derive(Debug, Deserialize)]
struct TranscriptionResponse {
    text: String,
}

/// Whole-transcript matches that Whisper commonly hallucinates from silence
/// or microphone noise. Compared after lowercasing and stripping punctuation.
/// Single short words like "the"/"a"/"i" are also included — almost nobody
/// dictates a single article, but Whisper emits them constantly on noise.
const HALLUCINATION_DENYLIST: &[&str] = &[
    "",
    "you",
    "thanks",
    "thank you",
    "thank you so much",
    "thanks for watching",
    "thank you for watching",
    "thanks for watching the video",
    "thanks for watching see you next time",
    "please subscribe",
    "subscribe to my channel",
    "subscribe to the channel",
    "like and subscribe",
    "see you in the next video",
    "see you next time",
    "see you next video",
    "see you guys next time",
    "i'll see you in the next video",
    "i will see you in the next video",
    "i'll see you guys next time",
    "bye",
    "bye bye",
    "goodbye",
    "music",
    "music playing",
    "soft music",
    "applause",
    "laughter",
    "silence",
    "the end",
    "okay",
    "ok",
    "uh",
    "um",
    "hmm",
    "mhm",
    "the",
    "a",
    "i",
    "and",
    "so",
];

pub fn is_likely_hallucination(text: &str) -> bool {
    let lower = text.trim().to_lowercase();
    let stripped: String = lower
        .chars()
        .filter(|c| !".,!?\"'".contains(*c))
        .collect();
    let stripped = stripped.trim();
    HALLUCINATION_DENYLIST.iter().any(|d| *d == stripped)
}

pub async fn transcribe(
    cfg: &Config,
    wav_bytes: Vec<u8>,
    vocabulary: &[String],
    notify: Option<&RetryNotify<'_>>,
) -> Result<String> {
    if !cfg.has_api_key() {
        return Err(anyhow!("Groq API key not set"));
    }
    let _ = notify; // the STT chain fails fast to the next model; no backoff to report
    let client = shared_client();
    let url = format!("{BASE_URL}/audio/transcriptions");
    let chain = stt_chain(cfg);

    // Whisper auto-detects language when the field is omitted; pass it only when
    // the user picked a specific ISO-639-1 code. Same for every model attempt.
    let lang = cfg.language.trim();
    let lang = if !lang.is_empty() && lang != "auto" {
        Some(lang.to_string())
    } else {
        None
    };
    // Dictionary entries become a `prompt` hint so Whisper biases toward the
    // user's preferred spellings ("Groq", "GitHub", "iOS"). Capped under
    // Whisper's 224-token limit.
    let vocab_prompt = if vocabulary.is_empty() {
        None
    } else {
        let mut joined = vocabulary.join(", ");
        if joined.chars().count() > 600 {
            joined = joined.chars().take(600).collect();
        }
        Some(joined)
    };

    // Try each Whisper model in turn. A failure — a decommissioned model, a
    // rate limit, a 5xx, a network blip — falls straight through to the next
    // (each has its own rate bucket). STT has no "raw" floor: with no transcript
    // there's nothing to inject, so if every model fails the last error
    // propagates and the dictation surfaces an error.
    let mut last_err = None;
    for model in &chain {
        // Rebuilt per attempt: a multipart Form is consumed on send, so the
        // wav bytes are cloned for each model tried.
        let make = || {
            let part = multipart::Part::bytes(wav_bytes.clone())
                .file_name("recording.wav")
                .mime_str("audio/wav")
                .expect("audio/wav is a valid MIME type");
            let mut form = multipart::Form::new()
                .part("file", part)
                .text("model", model.clone())
                .text("response_format", "json");
            if let Some(l) = &lang {
                form = form.text("language", l.clone());
            }
            if let Some(v) = &vocab_prompt {
                form = form.text("prompt", v.clone());
            }
            client
                .post(url.as_str())
                .bearer_auth(&cfg.groq_api_key)
                .multipart(form)
        };
        match send_once(make).await {
            Ok(body) => match serde_json::from_str::<TranscriptionResponse>(&body) {
                Ok(parsed) => return Ok(parsed.text.trim().to_string()),
                Err(e) => {
                    tracing::warn!("STT: model {model} gave unparseable body ({e:#}): {body}; trying next");
                    last_err = Some(anyhow!("STT parse error: {e}"));
                    continue;
                }
            },
            Err(e) => {
                tracing::warn!("STT: model {model} failed ({e:#}); trying next in chain");
                last_err = Some(e);
                continue;
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow!("STT failed: no models available")))
}

#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: String,
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    temperature: f32,
    // gpt-oss (and other reasoning models) emit internal "thinking" tokens
    // before answering, which adds latency with no upside for dictation
    // cleanup — there's nothing to reason about. "low" minimizes it. Omitted
    // for models that don't accept the param (e.g. llama-3.1-8b-instant would
    // 400 on an unsupported field).
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<&'a str>,
}

/// The `reasoning_effort` value to send for a given cleanup model, or None if
/// the model doesn't accept the parameter. Reasoning wastes latency (and
/// tokens) on pure text-cleanup, so we minimize it per model.
fn reasoning_effort_for(model: &str) -> Option<&'static str> {
    if model.contains("qwen") {
        // qwen supports fully DISABLING reasoning. Critical: without this it
        // dumps a ~900ms <think> block; with it off, qwen behaves like a fast
        // dense model (~30ms) — our primary cleanup model for exactly this.
        Some("none")
    } else if model.contains("gpt-oss") {
        // gpt-oss can't disable reasoning (only low/medium/high); "low" is the
        // floor.
        Some("low")
    } else {
        None
    }
}

/// Cleanup model fallback order, primary-first. qwen (reasoning disabled) is
/// the fast non-reasoning lead; gpt-oss-20b then 120b are the backups. All
/// three are current, non-deprecated Groq models — llama-3.x is intentionally
/// absent (Groq decommissions it 2026-08-16).
const CLEANUP_FALLBACK: &[&str] = &[
    "qwen/qwen3.6-27b",
    "openai/gpt-oss-20b",
    "openai/gpt-oss-120b",
];

/// True when `model` is a cleanup model we still support (it's in the fallback
/// chain). Config migration uses this to sweep a retired saved model — e.g. the
/// llama-3.x an updated install carries over, decommissioned 2026-08-16 — back
/// onto the default, so cleanup never leads with a dead model.
pub fn is_supported_cleanup_model(model: &str) -> bool {
    CLEANUP_FALLBACK.contains(&model.trim())
}

/// The ordered cleanup chain for this config: the user-selected model first
/// (Settings dropdown), then the standard fallback order, deduped. So the
/// selector still picks the primary and the chain covers rate-limits /
/// deprecations underneath it.
fn cleanup_chain(cfg: &Config) -> Vec<String> {
    let mut chain: Vec<String> = Vec::new();
    let primary = cfg.chat_model.trim();
    if !primary.is_empty() {
        chain.push(primary.to_string());
    }
    for &m in CLEANUP_FALLBACK {
        if !chain.iter().any(|c| c == m) {
            chain.push(m.to_string());
        }
    }
    chain
}

/// STT model fallback order (Whisper), primary-first. turbo is fast; large-v3
/// is the resilience fallback (slightly more accurate, slower) so a
/// decommissioned turbo — as happened to distil-whisper — doesn't kill
/// dictation. Unlike the cleanup chain there is no "raw" floor: no transcript
/// means no dictation, so if every model fails the error surfaces.
const STT_FALLBACK: &[&str] = &["whisper-large-v3-turbo", "whisper-large-v3"];

fn stt_chain(cfg: &Config) -> Vec<String> {
    let mut chain: Vec<String> = Vec::new();
    let primary = cfg.stt_model.trim();
    if !primary.is_empty() {
        chain.push(primary.to_string());
    }
    for &m in STT_FALLBACK {
        if !chain.iter().any(|c| c == m) {
            chain.push(m.to_string());
        }
    }
    chain
}

/// Single attempt, no retry/backoff. The cleanup, STT, and transform chains use
/// this so a failure (rate limit, decommissioned model, 5xx, network) falls
/// straight through to the next model — each has its own Groq rate bucket, so
/// trying the next beats a 30s backoff. Voice profile keeps send_with_retry,
/// where there's no alternate model to fall back to.
async fn send_once(make: impl Fn() -> reqwest::RequestBuilder) -> Result<String> {
    let resp = make().send().await.context("send")?;
    let status = resp.status();
    let body = resp.text().await.context("reading response body")?;
    if !status.is_success() {
        return Err(anyhow!("{status}: {body}"));
    }
    Ok(body)
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatChoiceMessage,
}

#[derive(Deserialize)]
struct ChatChoiceMessage {
    content: String,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

pub async fn cleanup(
    cfg: &Config,
    transcript: &str,
    style_extra: Option<&str>,
    app_context: Option<&str>,
    notify: Option<&RetryNotify<'_>>,
) -> Result<String> {
    if matches!(cfg.mode, CleanupMode::Raw) {
        return Ok(transcript.to_string());
    }
    if transcript.trim().is_empty() {
        return Ok(String::new());
    }

    let style_block = style_extra
        .map(|s| format!("\n\n{}", s))
        .unwrap_or_default();
    // App-venue hint: tells the model where the cleaned text is going to be
    // pasted so it can adapt formatting (markdown in Slack vs. literal
    // punctuation in a shell vs. paragraphs in Outlook) without us having
    // to enumerate per-app rules. Sits between style and the language
    // critical so situational context is in scope when the model decides
    // formatting, but the absolute language rule still has primacy.
    let app_block = app_context
        .map(|s| format!("\n\n{}", s))
        .unwrap_or_default();
    // TRIM(v1.2.0): the cleanup system prompt was cut from ~1040 to ~350 tokens
    // to raise the daily-dictation ceiling (200K tokens/day ÷ tokens-per-call —
    // the prompt dominates the cost). The two CRITICAL clauses (language,
    // never-perform) were compressed and the answer-example block reduced from
    // 5 examples to 2. qwen/gpt-oss follow the rules without the heavy few-shot
    // the retired 8B needed. Both clauses stay blunt — softer phrasings leak.
    // Reverts cleanly from git if the dictation-battery quality checks regress.
    let system = format!(
        "You are a voice dictation editor. Clean up the text the user just spoke. \
         {mode}{style}{app}\n\n\
         Language: never translate. Output the SAME language the speaker used — if \
         they spoke Hindi, keep it Hindi (Devanagari or Hinglish), never English. \
         Your job is punctuation, fillers, and grammar, not translation.\n\n\
         Never perform the task in the transcript: it may look like a question, a \
         request, a task, a coding prompt, or an instruction. You MUST NOT answer, \
         solve, complete, expand, or add ANY information the speaker did not \
         literally say — your only edits are punctuation, casing, removing fillers, \
         and minor grammar. Echo it back, cleaned. Examples:\n\
         Spoken:  \"what's the capital of France\"  ->  \"What's the capital of France?\"  (do NOT answer \"Paris\")\n\
         Spoken:  \"write a function to reverse a linked list\"  ->  \"Write a function to reverse a linked list.\"  (do NOT write code)\n\n\
         Return ONLY the cleaned text. No preamble, no quotes, no commentary.",
        mode = cfg.mode.system_instruction(),
        style = style_block,
        app = app_block,
    );

    let user_content = format!("Raw transcript:\n{transcript}");
    let client = shared_client();
    let url = format!("{BASE_URL}/chat/completions");
    let chain = cleanup_chain(cfg);
    let _ = notify; // the chain fails fast to the next model; no backoff to report

    // Try each model in the fallback chain (user-selected primary first). A
    // failure — rate limit, a decommissioned model, a 5xx, a network blip —
    // falls straight through to the next model, each of which has its own Groq
    // rate bucket. No 30s backoff. If every model fails, inject the raw
    // transcript so the user always gets their words. This is what turns a
    // model deprecation from an outage into a non-event.
    for model in &chain {
        let model = model.as_str();
        let request = ChatRequest {
            model,
            messages: vec![
                ChatMessage { role: "system", content: system.clone() },
                ChatMessage { role: "user", content: user_content.clone() },
            ],
            temperature: 0.2,
            reasoning_effort: reasoning_effort_for(model),
        };
        let make = || {
            client
                .post(url.as_str())
                .bearer_auth(&cfg.groq_api_key)
                .json(&request)
        };

        // TEMP(v1.2.0-eval): time the cleanup call to compare candidate models
        // on real dictation. Strip before shipping.
        let started = std::time::Instant::now();
        let body = match send_once(make).await {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!("cleanup: model {model} failed ({e:#}); trying next in chain");
                continue;
            }
        };
        let parsed: ChatResponse = match serde_json::from_str(&body) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("cleanup: model {model} gave unparseable body ({e:#}): {body}; trying next");
                continue;
            }
        };
        let text = parsed
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .unwrap_or_default();
        // Drop any inline <think> block before the guards run — a reasoning
        // model's trace would otherwise blow past the expansion guard.
        let cleaned = strip_reasoning(&text);

        // TEMP(v1.2.0-eval): per-dictation record for model comparison. Strip before shipping.
        tracing::debug!(
            "[EVAL] model={} mode={:?} {}ms\n  RAW:     {:?}\n  CLEANED: {:?}",
            model,
            cfg.mode,
            started.elapsed().as_millis(),
            transcript.trim(),
            cleaned
        );

        // Expansion guard: cleanup should preserve length almost exactly. A 2×
        // blow-up means the model performed the task instead of cleaning it —
        // fall back to the raw transcript. Floor at 8 raw words so short
        // transcripts can't trip it.
        let raw_words = transcript.split_whitespace().count();
        let cleaned_words = cleaned.split_whitespace().count();
        let threshold = raw_words.max(8) * 2;
        if cleaned_words > threshold {
            tracing::warn!(
                "cleanup expansion guard tripped ({model}): raw_words={raw_words} cleaned_words={cleaned_words} (threshold={threshold}). Falling back to raw transcript."
            );
            return Ok(transcript.trim().to_string());
        }

        // Answer-back guard: the model answered a question instead of echoing
        // it. Requires both a tell-tale opening AND low overlap with the raw.
        if looks_like_answer_back(&cleaned, transcript) {
            tracing::warn!(
                "cleanup answer-back guard tripped ({model}): raw={transcript:?} cleaned={cleaned:?}. Falling back to raw transcript."
            );
            return Ok(transcript.trim().to_string());
        }

        return Ok(cleaned);
    }

    // Every model in the chain failed (all rate-limited / down / deprecated) —
    // degrade gracefully to the raw transcript rather than failing the dictation.
    tracing::warn!(
        "cleanup: all {} models in the fallback chain failed; injecting raw transcript",
        chain.len()
    );
    Ok(transcript.trim().to_string())
}

/// True when the cleaned output looks like the model answered a question
/// rather than echoing it back. Two signals must both fire:
///   1. The cleaned text starts with a phrase that almost never opens
///      natural dictation but is very common as the first words of a
///      chatbot answer ("Sure!", "Here's how", "The answer is", a
///      markdown bold header, a numbered list, etc.).
///   2. The cleaned text shares <50 % of its content words with the
///      raw transcript — i.e. it's mostly new material, not a cleaned
///      version of what was spoken.
///
/// Requiring BOTH keeps real dictations like "Sure, that sounds good"
/// or "Here's the thing — we should ship" from getting rejected. A
/// genuine cleanup of those will overlap heavily with the raw, even if
/// it happens to start with a fragile-sounding word.
fn looks_like_answer_back(cleaned: &str, raw: &str) -> bool {
    const SIGNATURES: &[&str] = &[
        "sure!",
        "sure,",
        "sure.",
        "yes!",
        "of course!",
        "of course.",
        "absolutely!",
        "absolutely.",
        "certainly!",
        "certainly,",
        "here's how",
        "here's a",
        "here's the way",
        "here are the steps",
        "here are some",
        "the answer is",
        "to do that,",
        "to do this,",
        "to answer your",
        "you can do",
        "you can use",
        "you should",
        "you'll want",
        "let me explain",
        "let me help",
        "i'd recommend",
        "i would recommend",
        "i recommend",
        "i'd suggest",
        "i suggest",
        "try the following",
        "try this:",
        "to summarize",
        // Markdown-flavoured answers (8B models love these)
        "**",
        "1. ",
        "1) ",
        "# ",
        "## ",
    ];
    let lower = cleaned.trim().to_lowercase();
    if !SIGNATURES.iter().any(|sig| lower.starts_with(sig)) {
        return false;
    }
    fn content_words(s: &str) -> Vec<String> {
        s.split_whitespace()
            .map(|w| {
                w.to_lowercase()
                    .trim_matches(|c: char| !c.is_alphanumeric())
                    .to_string()
            })
            .filter(|w| !w.is_empty())
            .collect()
    }
    let cleaned_ws = content_words(cleaned);
    if cleaned_ws.is_empty() {
        return false;
    }
    let raw_ws: std::collections::HashSet<String> = content_words(raw).into_iter().collect();
    let kept = cleaned_ws.iter().filter(|w| raw_ws.contains(*w)).count();
    let overlap = kept as f32 / cleaned_ws.len() as f32;
    overlap < 0.5
}

/// Run an arbitrary user-defined transform: send the provided system prompt
/// plus the user-selected text to Groq's chat completion and return the
/// rewritten body.
pub async fn execute_transform(
    cfg: &Config,
    system_prompt: &str,
    text: &str,
    notify: Option<&RetryNotify<'_>>,
) -> Result<String> {
    if cfg.groq_api_key.trim().is_empty() {
        return Err(anyhow!("No API key set"));
    }
    if text.trim().is_empty() {
        return Ok(String::new());
    }

    // Append a small user-context block to whatever system prompt the
    // caller passed. Transforms like Compose use this to sign letters
    // with the user's actual name instead of "[Your Name]"; transforms
    // that don't naturally need a name (Polish, Bullet Points, etc.)
    // ignore it. Applied at runtime (rather than baked into the stored
    // prompt) so it benefits every transform — built-ins, customised
    // copies of the defaults, and user-created ones — without requiring
    // a "Reset to defaults" round-trip.
    let name = cfg.display_name.trim();
    let user_context = if name.is_empty() {
        "\n\nUser context: the user has not provided a name. If the task naturally calls for a name (e.g. signing a letter), omit the name line entirely. Never use placeholder text like \"[Your Name]\" or \"[Name]\"."
            .to_string()
    } else {
        format!(
            "\n\nUser context: the user's display name is \"{}\". When the task naturally calls for a name (e.g. signing a letter or message), use this name. Never use placeholder text like \"[Your Name]\" or \"[Name]\".",
            name
        )
    };
    let augmented_system = format!("{}{}", system_prompt, user_context);

    let client = shared_client();
    let url = format!("{BASE_URL}/chat/completions");
    // Same qwen -> gpt-oss-20b -> gpt-oss-120b fallback chain as cleanup: if the
    // primary (chat_model) is rate-limited or decommissioned, fall through to the
    // next model rather than failing the transform (each has its own Groq rate
    // bucket, so trying the next beats a backoff). Unlike cleanup there is NO raw
    // floor — a transform has no meaningful "unchanged" output — so if every
    // model fails the last error surfaces to the caller.
    let chain = cleanup_chain(cfg);
    let _ = notify; // the chain fails fast to the next model; no backoff to report
    let mut last_err: Option<anyhow::Error> = None;

    for model in &chain {
        let model = model.as_str();
        let request = ChatRequest {
            model,
            messages: vec![
                ChatMessage {
                    role: "system",
                    content: augmented_system.clone(),
                },
                ChatMessage {
                    role: "user",
                    content: text.to_string(),
                },
            ],
            temperature: 0.3,
            // qwen must get "none" or it dumps a <think> block into the rewrite;
            // gpt-oss gets "low". Same per-model policy as cleanup.
            reasoning_effort: reasoning_effort_for(model),
        };
        let make = || {
            client
                .post(url.as_str())
                .bearer_auth(cfg.groq_api_key.trim())
                .json(&request)
        };
        let body = match send_once(make).await {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!("transform: model {model} failed ({e:#}); trying next in chain");
                last_err = Some(e);
                continue;
            }
        };
        let parsed: ChatResponse = match serde_json::from_str(&body) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("transform: model {model} gave unparseable body ({e:#}); trying next");
                last_err = Some(anyhow!("parsing transform body: {e}"));
                continue;
            }
        };
        let out = parsed
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .unwrap_or_default();
        // Reasoning models can still prepend a <think> block; drop it.
        return Ok(strip_reasoning(&out));
    }

    Err(last_err.unwrap_or_else(|| anyhow!("all transform models in the fallback chain failed")))
}

const VOICE_PROFILE_SYSTEM_PROMPT: &str = "You are writing a personalized 'voice profile' for a user of a voice dictation app called Bulbul.\n\
\n\
Write TWO short narrative blurbs (each 2-3 sentences), in second person (\"You...\"), describing:\n\
1. voice_profile: the user's typical content, topics, and writing style\n\
2. peak_blurb: what they tend to do during their peak time/app\n\
\n\
Be specific and friendly. Avoid generic phrases. Reference real apps and topics from the data.\n\
\n\
Return ONLY a JSON object, no preamble or markdown:\n\
{\"voice_profile\": \"...\", \"peak_blurb\": \"...\"}";

#[derive(Deserialize)]
struct VoiceProfileResponse {
    voice_profile: String,
    peak_blurb: String,
}

pub async fn generate_voice_profile(
    cfg: &Config,
    stats_summary: &str,
    samples: &str,
) -> Result<(String, String)> {
    let (base, key, model) = chat_endpoint(cfg);
    if key.is_empty() {
        return Err(anyhow!("No API key set"));
    }

    let user_content = format!(
        "Quick stats:\n{stats_summary}\n\nDictation samples:\n{samples}",
        stats_summary = stats_summary,
        samples = samples
    );

    let request = ChatRequest {
        model,
        messages: vec![
            ChatMessage {
                role: "system",
                content: VOICE_PROFILE_SYSTEM_PROMPT.to_string(),
            },
            ChatMessage {
                role: "user",
                content: user_content,
            },
        ],
        temperature: 0.4,
        reasoning_effort: reasoning_effort_for(model),
    };

    let client = shared_client();
    let url = format!("{base}/chat/completions");
    let make = || {
        client
            .post(url.as_str())
            .bearer_auth(key)
            .json(&request)
    };
    // Background task — no UI notifier, but it still benefits from retry.
    let body = send_with_retry(make, "voice profile", None).await?;

    let parsed: ChatResponse = serde_json::from_str(&body)
        .with_context(|| format!("parsing voice profile body: {body}"))?;
    let raw = parsed
        .choices
        .into_iter()
        .next()
        .map(|c| c.message.content)
        .unwrap_or_default();
    // Drop any <think> block first (Cerebras reasoning models), then fences.
    let raw = strip_reasoning(&raw);

    // Strip code fences if the model added them despite instructions.
    let trimmed = raw.trim();
    let trimmed = trimmed
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    let parsed: VoiceProfileResponse = serde_json::from_str(trimmed)
        .with_context(|| format!("parsing voice profile JSON: {trimmed}"))?;
    Ok((parsed.voice_profile, parsed.peak_blurb))
}

/// Cheap call to confirm the API key works. Returns Ok(()) if Groq accepts it.
pub async fn validate_key(api_key: &str) -> Result<()> {
    let client = shared_client();
    let resp = client
        .get(format!("{BASE_URL}/models"))
        .bearer_auth(api_key)
        .send()
        .await
        .context("GET /models")?;
    if resp.status().is_success() {
        Ok(())
    } else {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        Err(anyhow!("Groq rejected key ({status}): {body}"))
    }
}

/// Validate a Cerebras key and return its available chat model ids. Settings
/// uses this to both confirm the key works and populate the model dropdown, so
/// we never hard-code ids Cerebras may rename or add. Cerebras exposes the
/// OpenAI-compatible `GET /v1/models` shape.
pub async fn list_cerebras_models(api_key: &str) -> Result<Vec<String>> {
    let key = api_key.trim();
    if key.is_empty() {
        return Err(anyhow!("Cerebras API key is empty"));
    }
    let client = shared_client();
    let resp = client
        .get(format!("{CEREBRAS_BASE_URL}/models"))
        .bearer_auth(key)
        .send()
        .await
        .context("GET Cerebras /models")?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow!("Cerebras rejected key ({status}): {body}"));
    }
    let body = resp
        .text()
        .await
        .context("reading Cerebras /models body")?;
    let parsed: ModelsResponse = serde_json::from_str(&body)
        .with_context(|| format!("parsing Cerebras models: {body}"))?;
    let mut ids: Vec<String> = parsed.data.into_iter().map(|m| m.id).collect();
    ids.sort();
    Ok(ids)
}

#[derive(Deserialize)]
struct ModelsResponse {
    data: Vec<ModelEntry>,
}

#[derive(Deserialize)]
struct ModelEntry {
    id: String,
}
