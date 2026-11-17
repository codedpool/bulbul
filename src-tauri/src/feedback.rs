// SPDX-License-Identifier: GPL-3.0-only
// Copyright (c) 2026 Romanch Roshan Singh

//! "Write to me": sends a short message from the person using Bulbul to the
//! developer.
//!
//! Design constraints:
//! - **Only what the person typed.** Name, message, and an email address only
//!   if they chose to include one. We add the app version and OS family so a
//!   bug report says which build it came from, and nothing else: no ID, no
//!   settings, no transcripts, no logs.
//! - **Not tied to the analytics setting.** Sending a message is something the
//!   person does on purpose, so it works whether or not analytics is on.
//! - **No keys in the app.** The message goes to bulbultypes.xyz, which
//!   validates it, rate-limits it and stores it. The app holds nothing that
//!   could read or write the database.
//! - **Shared by desktop and Android.** This module compiles for both.

use serde::Serialize;
use std::time::Duration;

const ENDPOINT: &str = "https://bulbultypes.xyz/api/feedback";
const TIMEOUT: Duration = Duration::from_secs(15);

// Mirrors the limits the server enforces, so the dialog can say what is wrong
// instantly instead of after a round trip.
const MAX_NAME: usize = 80;
const MAX_MESSAGE: usize = 2000;
const MAX_EMAIL: usize = 254;

#[derive(Serialize, Debug, PartialEq)]
struct Payload {
    name: String,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    email: Option<String>,
    app_version: String,
    os: &'static str,
}

fn os_label() -> &'static str {
    if cfg!(target_os = "android") {
        "android"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "other"
    }
}

fn looks_like_email(s: &str) -> bool {
    // Same idea as the server's check: something@domain.tld, no spaces, and
    // none of the characters that could change what a mailto: link does.
    if s.len() > MAX_EMAIL || s.chars().any(|c| c.is_whitespace() || "?&#%<>\"'".contains(c)) {
        return false;
    }
    let Some((local, domain)) = s.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && local.len() <= 64
        && !domain.contains('@')
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
}

fn build_payload(name: &str, message: &str, email: Option<&str>) -> Result<Payload, String> {
    let name = name.trim();
    let message = message.trim();
    let email = email.map(str::trim).filter(|e| !e.is_empty());

    if name.is_empty() {
        return Err("Please add your name.".into());
    }
    if name.chars().count() > MAX_NAME {
        return Err(format!("Name is too long ({MAX_NAME} characters at most)."));
    }
    if message.is_empty() {
        return Err("Please write a message.".into());
    }
    if message.chars().count() > MAX_MESSAGE {
        return Err(format!("Message is too long ({MAX_MESSAGE} characters at most)."));
    }
    if let Some(e) = email {
        if !looks_like_email(e) {
            return Err("That email address doesn't look right.".into());
        }
    }

    Ok(Payload {
        name: name.to_string(),
        message: message.to_string(),
        email: email.map(str::to_string),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        os: os_label(),
    })
}

/// Posts the payload and turns every failure into a sentence for the person
/// using the app.
async fn post_payload(url: &str, payload: &Payload) -> Result<(), String> {
    let client = reqwest::Client::new();
    let resp = client
        .post(url)
        .json(payload)
        .timeout(TIMEOUT)
        .send()
        .await
        .map_err(|e| {
            tracing::debug!("feedback send failed: {e:#}");
            "Couldn't reach Bulbul. Check your connection and try again.".to_string()
        })?;

    if resp.status().is_success() {
        return Ok(());
    }

    // The server answers with { ok: false, error: "..." } in plain language.
    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap_or_default();
    let msg = body
        .get("error")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| "Couldn't send your message. Please try again in a moment.".to_string());
    tracing::debug!("feedback rejected: {status}");
    Err(msg)
}

/// Sends the message. The `Err` text is written for the person using the app,
/// so the dialog can show it as is.
#[tauri::command]
pub async fn send_feedback(
    name: String,
    message: String,
    email: Option<String>,
) -> Result<(), String> {
    let payload = build_payload(&name, &message, email.as_deref())?;
    post_payload(ENDPOINT, &payload).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_and_accepts_a_normal_message() {
        let p = build_payload("  Ana ", " hello there \n", Some(" ana@example.com ")).unwrap();
        assert_eq!(p.name, "Ana");
        assert_eq!(p.message, "hello there");
        assert_eq!(p.email.as_deref(), Some("ana@example.com"));
    }

    #[test]
    fn email_is_optional_and_blank_means_none() {
        assert_eq!(build_payload("A", "m", None).unwrap().email, None);
        assert_eq!(build_payload("A", "m", Some("   ")).unwrap().email, None);
    }

    #[test]
    fn email_is_left_out_of_the_json_when_absent() {
        let json = serde_json::to_value(build_payload("A", "m", None).unwrap()).unwrap();
        assert!(json.get("email").is_none());
        assert!(json.get("app_version").is_some() && json.get("os").is_some());
    }

    #[test]
    fn nothing_else_is_sent() {
        let json = serde_json::to_value(build_payload("A", "m", Some("a@b.co")).unwrap()).unwrap();
        let mut keys: Vec<_> = json.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, ["app_version", "email", "message", "name", "os"]);
    }

    #[test]
    fn rejects_empty_and_oversized_fields() {
        assert!(build_payload("", "m", None).is_err());
        assert!(build_payload("A", "   ", None).is_err());
        assert!(build_payload(&"n".repeat(81), "m", None).is_err());
        assert!(build_payload("A", &"m".repeat(2001), None).is_err());
        assert!(build_payload("A", &"m".repeat(2000), None).is_ok());
        assert!(build_payload(&"n".repeat(80), "m", None).is_ok());
    }

    #[test]
    fn limits_count_characters_not_bytes() {
        // 2000 two-byte characters is within the limit.
        assert!(build_payload("A", &"é".repeat(2000), None).is_ok());
        assert!(build_payload("A", &"é".repeat(2001), None).is_err());
    }

    #[test]
    fn rejects_bad_emails() {
        for bad in [
            "nope",
            "a@b",
            "a b@c.de",
            "a@b.co?bcc=x@y.zz",
            "@b.co",
            "a@@b.co",
            "a@.co",
            "a@b.",
            "a@b.co#x",
        ] {
            assert!(build_payload("A", "m", Some(bad)).is_err(), "{bad} should be rejected");
        }
        for good in ["a@b.co", "first.last+tag@sub.example.org"] {
            assert!(build_payload("A", "m", Some(good)).is_ok(), "{good} should be accepted");
        }
    }

    // A throwaway one-shot HTTP server so the real request/response path runs.
    // Returns its URL and a handle that yields the raw request it received.
    fn serve_once(
        status_line: &'static str,
        body: &'static str,
    ) -> (String, std::thread::JoinHandle<String>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/api/feedback", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut buf: Vec<u8> = Vec::new();
            let mut chunk = [0u8; 4096];
            let sep: &[u8] = b"\r\n\r\n";
            loop {
                let n = sock.read(&mut chunk).unwrap();
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
                if let Some(h) = buf.windows(4).position(|w| w == sep) {
                    let head = String::from_utf8_lossy(&buf[..h]).to_ascii_lowercase();
                    let len = head
                        .lines()
                        .find_map(|l| l.strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap()))
                        .unwrap_or(0);
                    if buf.len() >= h + 4 + len {
                        break;
                    }
                }
            }
            let resp = format!(
                "HTTP/1.1 {status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            sock.write_all(resp.as_bytes()).unwrap();
            String::from_utf8_lossy(&buf).to_string()
        });
        (url, handle)
    }

    fn send(url: &str, p: &Payload) -> Result<(), String> {
        tauri::async_runtime::block_on(post_payload(url, p))
    }

    #[test]
    fn posts_the_json_the_server_expects() {
        let (url, server) = serve_once("200 OK", r#"{"ok":true}"#);
        let p = build_payload("Ana", "hello", Some("ana@example.com")).unwrap();
        assert!(send(&url, &p).is_ok());
        let raw = server.join().unwrap();
        assert!(raw.starts_with("POST /api/feedback"));
        assert!(raw.to_lowercase().contains("content-type: application/json"));
        let body = &raw[raw.find("\r\n\r\n").unwrap() + 4..];
        let v: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(v["name"], "Ana");
        assert_eq!(v["message"], "hello");
        assert_eq!(v["email"], "ana@example.com");
        assert!(v["app_version"].is_string() && v["os"].is_string());
    }

    #[test]
    fn shows_the_servers_own_words_when_it_says_no() {
        let (url, server) = serve_once(
            "429 Too Many Requests",
            r#"{"ok":false,"error":"You've sent a few messages already. Please try again later."}"#,
        );
        let p = build_payload("A", "m", None).unwrap();
        let err = send(&url, &p).unwrap_err();
        server.join().unwrap();
        assert_eq!(err, "You've sent a few messages already. Please try again later.");
    }

    #[test]
    fn falls_back_to_a_plain_message_when_the_server_gives_no_reason() {
        let (url, server) = serve_once("503 Service Unavailable", "not json at all");
        let p = build_payload("A", "m", None).unwrap();
        let err = send(&url, &p).unwrap_err();
        server.join().unwrap();
        assert!(err.contains("try again"), "{err}");
        assert!(!err.contains("not json"), "raw server text must not reach the person");
    }

    #[test]
    fn says_so_when_there_is_no_connection() {
        // Port 1 is never listening.
        let p = build_payload("A", "m", None).unwrap();
        let err = send("http://127.0.0.1:1/api/feedback", &p).unwrap_err();
        assert!(err.contains("Check your connection"), "{err}");
    }
}
