use crate::config::{CleanupMode, Config};
use anyhow::{anyhow, Context, Result};
use reqwest::multipart;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use std::time::Duration;

const BASE_URL: &str = "https://api.groq.com/openai/v1";

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
            tracing::warn!("Groq {label}: {status}; retry {attempt}/{MAX_ATTEMPTS} after {wait}s");
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
                "Groq is rate-limited right now. Wait a few seconds and try again."
            ));
        }
        if !status.is_success() {
            return Err(anyhow!("Groq {label} {status}: {body}"));
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
}

#[derive(Debug, Deserialize)]
struct TranscriptionResponse {
    text: String,
}

/// Whole-transcript matches that Whisper commonly hallucinates from silence
/// or microphone noise. Compared after lowercasing and stripping punctuation.
const HALLUCINATION_DENYLIST: &[&str] = &[
    "",
    "you",
    "thanks",
    "thank you",
    "thank you so much",
    "thanks for watching",
    "thanks for watching!",
    "thank you for watching",
    "please subscribe",
    "bye",
    "music",
    "okay",
    "ok",
    "uh",
    "um",
    "hmm",
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
    let client = shared_client();
    let url = format!("{BASE_URL}/audio/transcriptions");
    // Rebuilt per attempt: a multipart Form is consumed on send, so retries
    // need a fresh body (the wav bytes are cloned each time).
    let make = || {
        let part = multipart::Part::bytes(wav_bytes.clone())
            .file_name("recording.wav")
            .mime_str("audio/wav")
            .expect("audio/wav is a valid MIME type");
        let mut form = multipart::Form::new()
            .part("file", part)
            .text("model", cfg.stt_model.clone())
            .text("response_format", "json");
        // Whisper auto-detects when the field is omitted. Pass it only when
        // the user has chosen a specific ISO-639-1 code.
        let lang = cfg.language.trim();
        if !lang.is_empty() && lang != "auto" {
            form = form.text("language", lang.to_string());
        }
        // Dictionary entries become a `prompt` hint so Whisper biases toward
        // the user's preferred spellings (e.g. "Groq", "GitHub", "iOS") at
        // transcription time. Capped well under Whisper's 224-token limit.
        if !vocabulary.is_empty() {
            let mut joined = vocabulary.join(", ");
            if joined.chars().count() > 600 {
                joined = joined.chars().take(600).collect();
            }
            form = form.text("prompt", joined);
        }
        client
            .post(url.as_str())
            .bearer_auth(&cfg.groq_api_key)
            .multipart(form)
    };

    let body = send_with_retry(make, "STT", notify).await?;
    let parsed: TranscriptionResponse =
        serde_json::from_str(&body).with_context(|| format!("parsing STT body: {body}"))?;
    Ok(parsed.text.trim().to_string())
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
    // The language-preservation clause is load-bearing: without it,
    // `llama-3.1-8b-instant` happily translates Hindi (or any non-English)
    // input into English because its instruction-tuning is English-heavy
    // and "clean up" reads as "make English" when the input isn't already.
    // Wording is deliberately direct — softer phrasings leak.
    let system = format!(
        "You are a voice dictation editor. The user just spoke the following text. \
         {mode}{style}\n\n\
         CRITICAL: Never translate between languages. The output must be in the \
         same language as the speaker used. If the user spoke Hindi, output Hindi \
         (Devanagari script or romanized Hinglish in Latin script — either is \
         acceptable, but the vocabulary must remain Hindi, never replaced with \
         English equivalents). The same rule applies to every other non-English \
         language. Your job is punctuation, fillers, and grammar — not translation.\n\n\
         Return ONLY the cleaned text. No preamble, no quotes, no commentary.",
        mode = cfg.mode.system_instruction(),
        style = style_block,
    );

    let request = ChatRequest {
        model: cfg.chat_model.as_str(),
        messages: vec![
            ChatMessage {
                role: "system",
                content: system,
            },
            ChatMessage {
                role: "user",
                content: format!("Raw transcript:\n{transcript}"),
            },
        ],
        temperature: 0.2,
    };

    let client = shared_client();
    let url = format!("{BASE_URL}/chat/completions");
    let make = || {
        client
            .post(url.as_str())
            .bearer_auth(&cfg.groq_api_key)
            .json(&request)
    };
    let body = send_with_retry(make, "cleanup", notify).await?;
    let parsed: ChatResponse =
        serde_json::from_str(&body).with_context(|| format!("parsing chat body: {body}"))?;
    let text = parsed
        .choices
        .into_iter()
        .next()
        .map(|c| c.message.content)
        .unwrap_or_default();
    Ok(text.trim().to_string())
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
    if !cfg.has_api_key() {
        return Err(anyhow!("Groq API key not set"));
    }
    if text.trim().is_empty() {
        return Ok(String::new());
    }

    let request = ChatRequest {
        model: cfg.chat_model.as_str(),
        messages: vec![
            ChatMessage {
                role: "system",
                content: system_prompt.to_string(),
            },
            ChatMessage {
                role: "user",
                content: text.to_string(),
            },
        ],
        temperature: 0.3,
    };

    let client = shared_client();
    let url = format!("{BASE_URL}/chat/completions");
    let make = || {
        client
            .post(url.as_str())
            .bearer_auth(&cfg.groq_api_key)
            .json(&request)
    };
    let body = send_with_retry(make, "transform", notify).await?;
    let parsed: ChatResponse =
        serde_json::from_str(&body).with_context(|| format!("parsing transform body: {body}"))?;
    let out = parsed
        .choices
        .into_iter()
        .next()
        .map(|c| c.message.content)
        .unwrap_or_default();
    Ok(out.trim().to_string())
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
    if !cfg.has_api_key() {
        return Err(anyhow!("Groq API key not set"));
    }

    let user_content = format!(
        "Quick stats:\n{stats_summary}\n\nDictation samples:\n{samples}",
        stats_summary = stats_summary,
        samples = samples
    );

    let request = ChatRequest {
        model: cfg.chat_model.as_str(),
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
    };

    let client = shared_client();
    let url = format!("{BASE_URL}/chat/completions");
    let make = || {
        client
            .post(url.as_str())
            .bearer_auth(&cfg.groq_api_key)
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
