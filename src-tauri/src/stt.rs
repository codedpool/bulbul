// SPDX-License-Identifier: GPL-3.0-only
// Copyright (c) 2026 Romanch Roshan Singh

//! Speech-to-text provider router.
//!
//! One entry point, [`transcribe`], picks the backend the user chose (Groq,
//! any OpenAI-compatible transcription server, or — later — an on-device
//! model) so the dictation pipeline doesn't care which one produced the text.
//! When a non-Groq provider fails and the user opted in, the dictation is
//! retried once on Groq and the caller is told it happened.

use crate::config::{Config, SttProvider};
use crate::groq::{self, RetryNotify};
use anyhow::{anyhow, Context, Result};
use reqwest::multipart;
use serde::Deserialize;
use std::net::IpAddr;
use std::sync::OnceLock;
use std::time::Duration;

/// Long enough for a slow local model on a long recording; short enough that
/// a dead server doesn't hang dictation forever.
const ENDPOINT_TIMEOUT: Duration = Duration::from_secs(120);

/// How much of a server's error body to surface in a message.
const ERROR_BODY_CAP: usize = 300;

/// The transcript, plus which provider it was rescued from if the user's
/// chosen one failed and Groq stepped in.
pub struct Transcribed {
    pub text: String,
    pub fell_back_from: Option<SttProvider>,
}

pub fn provider_label(p: SttProvider) -> &'static str {
    match p {
        SttProvider::Groq => "Groq",
        SttProvider::OpenaiCompatible => "your transcription server",
        SttProvider::Local => "the on-device model",
    }
}

pub async fn transcribe(
    cfg: &Config,
    wav: Vec<u8>,
    vocabulary: &[String],
    notify: Option<&RetryNotify<'_>>,
) -> Result<Transcribed> {
    let primary = match cfg.stt_provider {
        SttProvider::Groq => {
            let text = groq::transcribe(cfg, wav, vocabulary, notify).await?;
            return Ok(Transcribed { text, fell_back_from: None });
        }
        SttProvider::OpenaiCompatible => endpoint_transcribe(cfg, wav.clone(), vocabulary).await,
        SttProvider::Local => match crate::local_stt::get() {
            Ok(local) => local.transcribe(cfg, &wav, groq::language_param(cfg).as_deref()).await,
            Err(e) => Err(e),
        },
    };

    match primary {
        Ok(text) => Ok(Transcribed { text, fell_back_from: None }),
        Err(e) if cfg.stt_cloud_fallback && cfg.has_api_key() => {
            tracing::warn!(
                "STT: {} failed ({e:#}); retrying this dictation on Groq",
                provider_label(cfg.stt_provider)
            );
            match groq::transcribe(cfg, wav, vocabulary, notify).await {
                Ok(text) => Ok(Transcribed { text, fell_back_from: Some(cfg.stt_provider) }),
                Err(e2) => Err(anyhow!(
                    "{} failed ({e:#}), and the Groq fallback failed too ({e2:#})",
                    provider_label(cfg.stt_provider)
                )),
            }
        }
        Err(e) => Err(e),
    }
}

// ---------- OpenAI-compatible endpoint ----------

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(ENDPOINT_TIMEOUT)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    })
}

/// Turns what the user typed into the full transcription URL. Accepts the
/// base (`http://localhost:11434/v1`) or the full path, with or without a
/// trailing slash.
fn endpoint_url(input: &str) -> Result<reqwest::Url> {
    endpoint_url_for(input, "/audio/transcriptions")
}

/// Same, for any OpenAI-style path (`/audio/transcriptions`,
/// `/chat/completions`): appends `suffix` unless the input already ends in it.
pub(crate) fn endpoint_url_for(input: &str, suffix: &str) -> Result<reqwest::Url> {
    let trimmed = input.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(anyhow!("No server address set. Enter your server's URL in Settings."));
    }
    if !trimmed.contains("://") {
        return Err(anyhow!(
            "The server address needs to start with http:// or https:// (got \"{trimmed}\")."
        ));
    }
    let full = if trimmed.ends_with(suffix) {
        trimmed.to_string()
    } else {
        format!("{trimmed}{suffix}")
    };
    let url = reqwest::Url::parse(&full)
        .map_err(|e| anyhow!("The server address isn't a valid URL ({e})."))?;
    match url.scheme() {
        "http" | "https" => Ok(url),
        other => Err(anyhow!("Only http:// and https:// servers are supported (got {other}://).")),
    }
}

/// Hosts where sending a key over plain http is acceptable: this machine or
/// the user's own network.
fn is_private_host(host: &str) -> bool {
    let h = host.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
    if h == "localhost" || h.ends_with(".localhost") || h.ends_with(".local") {
        return true;
    }
    if let Ok(ip) = h.parse::<IpAddr>() {
        return match ip {
            IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
            IpAddr::V6(v6) => {
                v6.is_loopback()
                    || (v6.segments()[0] & 0xfe00) == 0xfc00 // unique local fc00::/7
                    || (v6.segments()[0] & 0xffc0) == 0xfe80 // link local fe80::/10
            }
        };
    }
    // A bare LAN hostname like "ollama-box" has no dots.
    !h.contains('.')
}

/// Refuse to send a bearer key unencrypted across the internet.
pub(crate) fn check_key_transport(url: &reqwest::Url, has_key: bool) -> Result<()> {
    if has_key && url.scheme() == "http" && !url.host_str().is_some_and(is_private_host) {
        return Err(anyhow!(
            "Your server key would be sent unencrypted. Use https:// for servers outside your own network."
        ));
    }
    Ok(())
}

#[derive(Deserialize)]
struct EndpointResponse {
    text: String,
}

/// Servers that ignore `response_format=json` may answer with bare text.
fn parse_transcription_body(body: &str) -> Result<String> {
    let t = body.trim();
    if let Ok(parsed) = serde_json::from_str::<EndpointResponse>(t) {
        return Ok(parsed.text.trim().to_string());
    }
    if !t.is_empty() && !t.starts_with('{') && !t.starts_with('<') {
        return Ok(t.to_string());
    }
    Err(anyhow!(
        "The server's reply wasn't a transcript: {}",
        snippet(t)
    ))
}

fn snippet(s: &str) -> String {
    let s = s.trim();
    if s.chars().count() <= ERROR_BODY_CAP {
        s.to_string()
    } else {
        let cut: String = s.chars().take(ERROR_BODY_CAP).collect();
        format!("{cut}…")
    }
}

async fn endpoint_transcribe(cfg: &Config, wav: Vec<u8>, vocabulary: &[String]) -> Result<String> {
    let url = endpoint_url(&cfg.stt_endpoint_url)?;
    let model = cfg.stt_endpoint_model.trim();
    if model.is_empty() {
        return Err(anyhow!("No model name set for your transcription server."));
    }
    let key = cfg.stt_endpoint_key.trim();
    check_key_transport(&url, !key.is_empty())?;

    let part = multipart::Part::bytes(wav)
        .file_name("recording.wav")
        .mime_str("audio/wav")
        .context("building the audio upload")?;
    let mut form = multipart::Form::new()
        .part("file", part)
        .text("model", model.to_string())
        .text("response_format", "json");
    if let Some(l) = groq::language_param(cfg) {
        form = form.text("language", l);
    }
    if let Some(v) = groq::vocab_prompt(vocabulary) {
        form = form.text("prompt", v);
    }

    let host = url.host_str().unwrap_or("the server").to_string();
    let mut req = client().post(url).multipart(form);
    if !key.is_empty() {
        req = req.bearer_auth(key);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| anyhow!("Couldn't reach {host}: {}", e.without_url()))?;
    let status = resp.status();
    let body = resp.text().await.context("reading the server's reply")?;
    if status.is_success() {
        return parse_transcription_body(&body);
    }
    Err(match status.as_u16() {
        401 | 403 => anyhow!("{host} rejected the key ({status}). Check it in Settings."),
        404 => anyhow!(
            "{host} has no /audio/transcriptions endpoint, or doesn't know that model ({status}): {}",
            snippet(&body)
        ),
        _ => anyhow!("{host} returned {status}: {}", snippet(&body)),
    })
}

// ---------- "Test connection" ----------

/// Half a second of 16 kHz mono silence as a WAV: enough for a server to
/// accept the request and run its model without costing anything.
fn silent_wav() -> Vec<u8> {
    const RATE: u32 = 16_000;
    let samples = (RATE / 2) as usize;
    let data_len = (samples * 2) as u32;
    let mut w = Vec::with_capacity(44 + data_len as usize);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data_len).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    w.extend_from_slice(&1u16.to_le_bytes()); // PCM
    w.extend_from_slice(&1u16.to_le_bytes()); // mono
    w.extend_from_slice(&RATE.to_le_bytes());
    w.extend_from_slice(&(RATE * 2).to_le_bytes()); // byte rate
    w.extend_from_slice(&2u16.to_le_bytes()); // block align
    w.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data_len.to_le_bytes());
    w.resize(w.len() + data_len as usize, 0);
    w
}

/// Sends a short silent clip through the real transcription path, so a pass
/// means the address, key and model name all work together.
pub async fn test_transcription_endpoint(url: &str, model: &str, key: &str) -> Result<()> {
    let mut cfg = Config::default();
    cfg.stt_provider = SttProvider::OpenaiCompatible;
    cfg.stt_endpoint_url = url.to_string();
    cfg.stt_endpoint_model = model.to_string();
    cfg.stt_endpoint_key = key.to_string();
    endpoint_transcribe(&cfg, silent_wav(), &[]).await.map(|_| ())
}

/// Asks a chat server for a one-word reply, through the same target the
/// cleanup stage uses.
pub async fn test_chat_endpoint(url: &str, model: &str, key: &str) -> Result<()> {
    let mut cfg = Config::default();
    cfg.llm_provider = crate::config::LlmProvider::OpenaiCompatible;
    cfg.llm_endpoint_url = url.to_string();
    cfg.llm_endpoint_model = model.to_string();
    cfg.llm_endpoint_key = key.to_string();
    let target = groq::llm_target(&cfg)?;
    let model = target.chain.first().cloned().unwrap_or_default();
    let body = serde_json::json!({
        "model": model,
        "messages": [{ "role": "user", "content": "Reply with the single word OK." }],
        "max_tokens": 8,
    });
    let host = reqwest::Url::parse(&target.url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| "the server".to_string());
    let resp = target
        .auth(client().post(target.url.as_str()).json(&body))
        .send()
        .await
        .map_err(|e| anyhow!("Couldn't reach {host}: {}", e.without_url()))?;
    let status = resp.status();
    let text = resp.text().await.context("reading the server's reply")?;
    if !status.is_success() {
        return Err(match status.as_u16() {
            401 | 403 => anyhow!("{host} rejected the key ({status}). Check it and try again."),
            404 => anyhow!(
                "{host} has no /chat/completions endpoint, or doesn't know that model ({status}): {}",
                snippet(&text)
            ),
            _ => anyhow!("{host} returned {status}: {}", snippet(&text)),
        });
    }
    let v: serde_json::Value =
        serde_json::from_str(&text).map_err(|_| anyhow!("The server's reply wasn't a chat completion: {}", snippet(&text)))?;
    if v["choices"][0]["message"]["content"].is_string() {
        Ok(())
    } else {
        Err(anyhow!("The server's reply wasn't a chat completion: {}", snippet(&text)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn endpoint_url_accepts_base_and_full_forms() {
        for input in [
            "http://localhost:11434/v1",
            "http://localhost:11434/v1/",
            "  http://localhost:11434/v1  ",
            "http://localhost:11434/v1/audio/transcriptions",
        ] {
            assert_eq!(
                endpoint_url(input).unwrap().as_str(),
                "http://localhost:11434/v1/audio/transcriptions",
                "input: {input:?}"
            );
        }
        assert_eq!(
            endpoint_url("https://api.example.com/v1").unwrap().as_str(),
            "https://api.example.com/v1/audio/transcriptions"
        );
    }

    #[test]
    fn endpoint_url_rejects_unusable_input_with_a_helpful_message() {
        assert!(endpoint_url("").unwrap_err().to_string().contains("No server address"));
        assert!(endpoint_url("   ").is_err());
        assert!(endpoint_url("localhost:11434/v1")
            .unwrap_err()
            .to_string()
            .contains("http://"));
        assert!(endpoint_url("ftp://host/v1").is_err());
    }

    #[test]
    fn endpoint_url_for_handles_the_chat_path() {
        assert_eq!(
            endpoint_url_for("http://localhost:1234/v1", "/chat/completions").unwrap().as_str(),
            "http://localhost:1234/v1/chat/completions"
        );
        assert_eq!(
            endpoint_url_for("http://localhost:1234/v1/chat/completions/", "/chat/completions")
                .unwrap()
                .as_str(),
            "http://localhost:1234/v1/chat/completions"
        );
    }

    #[test]
    fn private_hosts_are_recognised() {
        for h in [
            "localhost", "127.0.0.1", "10.0.0.5", "192.168.1.20", "172.16.4.4", "169.254.1.1",
            "[::1]", "::1", "fd12:3456::1", "fe80::1", "mybox", "ollama.local",
        ] {
            assert!(is_private_host(h), "{h} should count as private");
        }
        for h in ["api.openai.com", "8.8.8.8", "203.0.113.9", "example.com", "2001:db8::1"] {
            assert!(!is_private_host(h), "{h} should count as public");
        }
    }

    #[test]
    fn keys_are_not_sent_over_plain_http_to_the_internet() {
        let u = |s: &str| endpoint_url(s).unwrap();
        assert!(check_key_transport(&u("http://localhost:11434/v1"), true).is_ok());
        assert!(check_key_transport(&u("http://192.168.1.9:8080/v1"), true).is_ok());
        assert!(check_key_transport(&u("https://api.example.com/v1"), true).is_ok());
        assert!(check_key_transport(&u("http://api.example.com/v1"), false).is_ok());
        let err = check_key_transport(&u("http://api.example.com/v1"), true).unwrap_err();
        assert!(err.to_string().contains("https://"));
    }

    #[test]
    fn transcription_bodies_parse_json_and_bare_text() {
        assert_eq!(parse_transcription_body(r#"{"text":"  hello world "}"#).unwrap(), "hello world");
        assert_eq!(parse_transcription_body("hello world\n").unwrap(), "hello world");
        assert!(parse_transcription_body("").is_err());
        assert!(parse_transcription_body(r#"{"error":"nope"}"#).is_err());
        assert!(parse_transcription_body("<html>502</html>").is_err());
    }

    #[test]
    fn long_error_bodies_are_cut() {
        let long = "x".repeat(1000);
        let s = snippet(&long);
        assert!(s.chars().count() <= ERROR_BODY_CAP + 1);
        assert!(s.ends_with('…'));
    }

    /// Serves exactly one HTTP request, replying with `status_line` + `body`,
    /// and returns the raw bytes the client sent.
    async fn serve_once(status_line: &'static str, body: &'static str) -> (u16, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = Vec::new();
            let mut chunk = [0u8; 8192];
            // Read headers, then the declared body length.
            loop {
                let n = sock.read(&mut chunk).await.unwrap();
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
                let text = String::from_utf8_lossy(&buf).to_string();
                if let Some(h_end) = text.find("\r\n\r\n") {
                    let len = text[..h_end]
                        .lines()
                        .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0)))
                        .unwrap_or(0);
                    if buf.len() >= h_end + 4 + len {
                        break;
                    }
                }
            }
            let reply = format!(
                "HTTP/1.1 {status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            sock.write_all(reply.as_bytes()).await.unwrap();
            let _ = sock.shutdown().await;
            String::from_utf8_lossy(&buf).to_string()
        });
        (port, handle)
    }

    fn cfg_for(port: u16) -> Config {
        let mut cfg = Config::default();
        cfg.stt_provider = SttProvider::OpenaiCompatible;
        cfg.stt_endpoint_url = format!("http://127.0.0.1:{port}/v1");
        cfg.stt_endpoint_model = "whisper-test".into();
        cfg.language = "hi".into();
        cfg
    }

    #[tokio::test]
    async fn endpoint_request_is_built_and_the_transcript_returned() {
        let (port, server) = serve_once("200 OK", r#"{"text":" namaste duniya "}"#).await;
        let mut cfg = cfg_for(port);
        cfg.stt_endpoint_key = "sekret".into();
        let out = transcribe(&cfg, vec![1, 2, 3, 4], &["Bulbul".into(), "Groq".into()], None)
            .await
            .unwrap();
        assert_eq!(out.text, "namaste duniya");
        assert!(out.fell_back_from.is_none());

        let req = server.await.unwrap();
        let lower = req.to_ascii_lowercase();
        assert!(req.starts_with("POST /v1/audio/transcriptions"), "path: {}", &req[..60.min(req.len())]);
        assert!(lower.contains("authorization: bearer sekret"));
        assert!(req.contains(r#"name="model""#) && req.contains("whisper-test"));
        assert!(req.contains(r#"name="language""#) && req.contains("\r\nhi\r\n"));
        assert!(req.contains(r#"name="prompt""#) && req.contains("Bulbul, Groq"));
        assert!(req.contains(r#"name="file""#) && req.contains("recording.wav"));
    }

    #[tokio::test]
    async fn no_key_means_no_authorization_header_and_auto_language_is_omitted() {
        let (port, server) = serve_once("200 OK", r#"{"text":"ok"}"#).await;
        let mut cfg = cfg_for(port);
        cfg.language = "auto".into();
        let out = transcribe(&cfg, vec![0; 16], &[], None).await.unwrap();
        assert_eq!(out.text, "ok");
        let req = server.await.unwrap();
        assert!(!req.to_ascii_lowercase().contains("authorization:"));
        assert!(!req.contains(r#"name="language""#));
        assert!(!req.contains(r#"name="prompt""#));
    }

    #[tokio::test]
    async fn server_errors_become_readable_messages() {
        let (port, _s) = serve_once("401 Unauthorized", r#"{"error":"bad key"}"#).await;
        let err = transcribe(&cfg_for(port), vec![0; 8], &[], None).await.err().unwrap();
        assert!(err.to_string().contains("rejected the key"), "{err}");

        let (port, _s) = serve_once("404 Not Found", "no such model").await;
        let err = transcribe(&cfg_for(port), vec![0; 8], &[], None).await.err().unwrap();
        let msg = err.to_string();
        assert!(msg.contains("/audio/transcriptions") && msg.contains("no such model"), "{msg}");
    }

    #[tokio::test]
    async fn unreachable_server_without_a_groq_key_just_errors() {
        // Nothing is listening here.
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        let mut cfg = cfg_for(port);
        cfg.stt_cloud_fallback = true; // fallback on, but no Groq key to fall back to
        let err = transcribe(&cfg, vec![0; 8], &[], None).await.err().unwrap();
        assert!(err.to_string().contains("Couldn't reach"), "{err}");
    }

    #[tokio::test]
    async fn local_provider_without_a_downloaded_model_says_so() {
        // Keep the test out of the real app data folder.
        let dir = std::env::temp_dir().join(format!("bulbul-local-stt-test-{}", std::process::id()));
        std::env::set_var("BULBUL_LOCAL_STT_DIR", &dir);

        let mut cfg = Config::default();
        cfg.stt_provider = SttProvider::Local;
        cfg.local_stt_model = "whisper-small".into();
        let err = transcribe(&cfg, vec![0; 8], &[], None).await.err().unwrap();
        assert!(err.to_string().contains("isn't downloaded yet"), "{err}");

        cfg.local_stt_model = String::new();
        let err = transcribe(&cfg, vec![0; 8], &[], None).await.err().unwrap();
        assert!(err.to_string().contains("No on-device model is selected"), "{err}");
    }

    #[test]
    fn the_silent_clip_is_a_valid_half_second_wav() {
        let w = silent_wav();
        assert_eq!(&w[0..4], b"RIFF");
        assert_eq!(&w[8..12], b"WAVE");
        assert_eq!(w.len(), 44 + 16_000); // 0.5 s * 16 kHz * 2 bytes
        let riff_len = u32::from_le_bytes([w[4], w[5], w[6], w[7]]) as usize;
        assert_eq!(riff_len, w.len() - 8);
        assert!(w[44..].iter().all(|b| *b == 0));
    }

    #[tokio::test]
    async fn test_connection_passes_against_a_working_transcription_server() {
        let (port, server) = serve_once("200 OK", r#"{"text":""}"#).await;
        test_transcription_endpoint(&format!("http://127.0.0.1:{port}/v1"), "whisper-test", "")
            .await
            .unwrap();
        let req = server.await.unwrap();
        assert!(req.starts_with("POST /v1/audio/transcriptions"));
        assert!(req.contains("whisper-test"));
    }

    #[tokio::test]
    async fn test_connection_reports_a_wrong_key_clearly() {
        let (port, _s) = serve_once("401 Unauthorized", "{}").await;
        let err = test_transcription_endpoint(&format!("http://127.0.0.1:{port}/v1"), "m", "bad")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("rejected the key"), "{err}");
    }

    #[tokio::test]
    async fn chat_test_passes_and_fails_like_the_real_path() {
        let (port, server) = serve_once("200 OK", CHAT_REPLY).await;
        test_chat_endpoint(&format!("http://127.0.0.1:{port}/v1"), "qwen2.5-7b", "")
            .await
            .unwrap();
        assert!(server.await.unwrap().starts_with("POST /v1/chat/completions"));

        let (port, _s) = serve_once("200 OK", r#"{"unexpected":true}"#).await;
        let err = test_chat_endpoint(&format!("http://127.0.0.1:{port}/v1"), "m", "")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("wasn't a chat completion"), "{err}");

        let (port, _s) = serve_once("404 Not Found", "model not found").await;
        let err = test_chat_endpoint(&format!("http://127.0.0.1:{port}/v1"), "m", "")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("model not found"), "{err}");
    }

    // ---------- self-hosted LLM for the text stages ----------

    fn llm_cfg_for(port: u16) -> Config {
        let mut cfg = Config::default();
        cfg.llm_provider = crate::config::LlmProvider::OpenaiCompatible;
        cfg.llm_endpoint_url = format!("http://127.0.0.1:{port}/v1");
        cfg.llm_endpoint_model = "qwen2.5-7b".into();
        cfg.mode = crate::config::CleanupMode::Clean;
        cfg
    }

    const CHAT_REPLY: &str = r#"{"choices":[{"message":{"content":"Hello, world."}}]}"#;

    #[tokio::test]
    async fn cleanup_runs_on_a_self_hosted_llm_without_groq_tuning() {
        let (port, server) = serve_once("200 OK", CHAT_REPLY).await;
        let out = groq::cleanup(&llm_cfg_for(port), "hello world", None, None, None)
            .await
            .unwrap();
        assert_eq!(out, "Hello, world.");
        let req = server.await.unwrap();
        assert!(req.starts_with("POST /v1/chat/completions"), "{}", &req[..50.min(req.len())]);
        assert!(req.contains(r#""model":"qwen2.5-7b""#));
        assert!(req.contains("max_tokens"));
        assert!(!req.contains("reasoning_effort"), "Groq-only field must not be sent");
        assert!(!req.to_ascii_lowercase().contains("authorization:"));
    }

    #[tokio::test]
    async fn self_hosted_llm_key_is_sent_as_a_bearer_token() {
        let (port, server) = serve_once("200 OK", CHAT_REPLY).await;
        let mut cfg = llm_cfg_for(port);
        cfg.llm_endpoint_key = "lm-key".into();
        groq::cleanup(&cfg, "hello world", None, None, None).await.unwrap();
        let req = server.await.unwrap().to_ascii_lowercase();
        assert!(req.contains("authorization: bearer lm-key"));
    }

    #[tokio::test]
    async fn cleanup_falls_back_to_the_raw_transcript_when_the_server_fails() {
        let (port, _s) = serve_once("500 Internal Server Error", "boom").await;
        let out = groq::cleanup(&llm_cfg_for(port), "hello world", None, None, None)
            .await
            .unwrap();
        assert_eq!(out, "hello world");
    }

    #[tokio::test]
    async fn cleanup_uses_the_raw_transcript_when_the_llm_is_not_configured() {
        let mut cfg = Config::default();
        cfg.llm_provider = crate::config::LlmProvider::OpenaiCompatible;
        cfg.mode = crate::config::CleanupMode::Clean;
        let out = groq::cleanup(&cfg, "hello world", None, None, None).await.unwrap();
        assert_eq!(out, "hello world");
    }

    #[tokio::test]
    async fn transforms_run_on_a_self_hosted_llm() {
        let (port, server) = serve_once("200 OK", CHAT_REPLY).await;
        let out = groq::execute_transform(&llm_cfg_for(port), "Make it formal.", "hey", None)
            .await
            .unwrap();
        assert_eq!(out, "Hello, world.");
        let req = server.await.unwrap();
        assert!(req.starts_with("POST /v1/chat/completions"));
        assert!(req.contains("Make it formal."));
    }

    #[tokio::test]
    async fn commands_run_on_a_self_hosted_llm_with_the_selection() {
        let (port, server) = serve_once("200 OK", CHAT_REPLY).await;
        let out = groq::execute_command(&llm_cfg_for(port), "make it formal", Some("hey there"))
            .await
            .unwrap();
        assert_eq!(out, "Hello, world.");
        let req = server.await.unwrap();
        assert!(req.starts_with("POST /v1/chat/completions"));
        assert!(req.contains("make it formal"));
        assert!(req.contains("hey there"));
        assert!(req.contains("selected_text"));
    }

    #[tokio::test]
    async fn commands_without_a_selection_still_reach_the_llm() {
        let (port, server) = serve_once("200 OK", CHAT_REPLY).await;
        let out = groq::execute_command(&llm_cfg_for(port), "write a short greeting", None)
            .await
            .unwrap();
        assert_eq!(out, "Hello, world.");
        let req = server.await.unwrap();
        assert!(req.contains("write a short greeting"));
        assert!(req.contains("Nothing is selected"));
    }

    #[tokio::test]
    async fn a_failing_server_surfaces_a_command_error() {
        let (port, _s) = serve_once("500 Internal Server Error", "boom").await;
        assert!(groq::execute_command(&llm_cfg_for(port), "make it formal", Some("hey")).await.is_err());
    }

    #[tokio::test]
    async fn transforms_surface_a_failed_self_hosted_server() {
        let (port, _s) = serve_once("500 Internal Server Error", "boom").await;
        assert!(groq::execute_transform(&llm_cfg_for(port), "Make it formal.", "hey", None)
            .await
            .is_err());
    }
}
