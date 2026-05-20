use crate::config::{CleanupMode, Config};
use anyhow::{anyhow, Context, Result};
use reqwest::multipart;
use serde::{Deserialize, Serialize};

const BASE_URL: &str = "https://api.groq.com/openai/v1";

#[derive(Debug, Deserialize)]
struct TranscriptionResponse {
    text: String,
}

pub async fn transcribe(cfg: &Config, wav_bytes: Vec<u8>) -> Result<String> {
    if !cfg.has_api_key() {
        return Err(anyhow!("Groq API key not set"));
    }
    let client = reqwest::Client::new();
    let part = multipart::Part::bytes(wav_bytes)
        .file_name("recording.wav")
        .mime_str("audio/wav")?;
    let form = multipart::Form::new()
        .part("file", part)
        .text("model", cfg.stt_model.clone())
        .text("response_format", "json");

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
