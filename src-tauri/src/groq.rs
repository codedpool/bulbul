use crate::config::{CleanupMode, Config};
use anyhow::{anyhow, Context, Result};
use reqwest::multipart;
use serde::{Deserialize, Serialize};

const BASE_URL: &str = "https://api.groq.com/openai/v1";

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

pub async fn transcribe(cfg: &Config, wav_bytes: Vec<u8>) -> Result<String> {
    if !cfg.has_api_key() {
        return Err(anyhow!("Groq API key not set"));
    }
    let client = reqwest::Client::new();
    let part = multipart::Part::bytes(wav_bytes)
        .file_name("recording.wav")
        .mime_str("audio/wav")?;
    let mut form = multipart::Form::new()
        .part("file", part)
        .text("model", cfg.stt_model.clone())
        .text("response_format", "json");
    // Whisper auto-detects when the field is omitted. Pass it only when the
    // user has chosen a specific ISO-639-1 code.
    let lang = cfg.language.trim();
    if !lang.is_empty() && lang != "auto" {
        form = form.text("language", lang.to_string());
    }

    let resp = client
        .post(format!("{BASE_URL}/audio/transcriptions"))
        .bearer_auth(&cfg.groq_api_key)
        .multipart(form)
        .send()
        .await
        .context("POST /audio/transcriptions")?;

    let status = resp.status();
    let body = resp.text().await.context("reading STT response body")?;
    if !status.is_success() {
        return Err(anyhow!("Groq STT {status}: {body}"));
    }
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

pub async fn cleanup(cfg: &Config, transcript: &str) -> Result<String> {
    if matches!(cfg.mode, CleanupMode::Raw) {
        return Ok(transcript.to_string());
    }
    if transcript.trim().is_empty() {
        return Ok(String::new());
    }

    let system = format!(
        "You are a voice dictation editor. The user just spoke the following text. \
         {mode}\n\n\
         Return ONLY the cleaned text. No preamble, no quotes, no commentary.",
        mode = cfg.mode.system_instruction()
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

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{BASE_URL}/chat/completions"))
        .bearer_auth(&cfg.groq_api_key)
        .json(&request)
        .send()
        .await
        .context("POST /chat/completions")?;

    let status = resp.status();
    let body = resp.text().await.context("reading cleanup response body")?;
    if !status.is_success() {
        return Err(anyhow!("Groq chat {status}: {body}"));
    }
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

const POLISH_SYSTEM_PROMPT: &str = "You are a writing editor. Polish the user's text:\n\
- Fix grammar, spelling, and punctuation errors.\n\
- Improve flow and clarity.\n\
- Preserve the original meaning, tone, and rough length — do not add new ideas.\n\
- Match the original register (casual stays casual, formal stays formal).\n\
\n\
Return ONLY the polished text. No preamble, no quotes around the output, no commentary.";

pub async fn polish(cfg: &Config, text: &str) -> Result<String> {
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
                content: POLISH_SYSTEM_PROMPT.to_string(),
            },
            ChatMessage {
                role: "user",
                content: text.to_string(),
            },
        ],
        temperature: 0.3,
    };

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{BASE_URL}/chat/completions"))
        .bearer_auth(&cfg.groq_api_key)
        .json(&request)
        .send()
        .await
        .context("POST /chat/completions (polish)")?;

    let status = resp.status();
    let body = resp.text().await.context("reading polish response body")?;
    if !status.is_success() {
        return Err(anyhow!("Groq polish {status}: {body}"));
    }
    let parsed: ChatResponse =
        serde_json::from_str(&body).with_context(|| format!("parsing polish body: {body}"))?;
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

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{BASE_URL}/chat/completions"))
        .bearer_auth(&cfg.groq_api_key)
        .json(&request)
        .send()
        .await
        .context("POST /chat/completions (voice profile)")?;

    let status = resp.status();
    let body = resp.text().await.context("reading voice profile body")?;
    if !status.is_success() {
        return Err(anyhow!("Groq voice {status}: {body}"));
    }

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
    let client = reqwest::Client::new();
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
