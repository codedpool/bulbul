// SPDX-License-Identifier: GPL-3.0-only
// Copyright (c) 2026 Romanch Roshan Singh

//! The conversation between the app and the speech worker program.
//!
//! Messages travel over the worker's stdin/stdout as *frames*:
//!
//! ```text
//! [u32 json_len, little endian][u32 payload_len, little endian][json][payload]
//! ```
//!
//! The JSON is a [`Request`] or [`Response`]. The optional payload carries
//! audio as raw little-endian `f32` samples, which is far more compact and
//! faster than writing every sample as JSON text.
//!
//! Frame sizes are capped on both sides, so a corrupted or hostile stream
//! can't make either program allocate unbounded memory.

use std::collections::BTreeMap;
use std::io::{Read, Write};

use serde::{Deserialize, Serialize};

use crate::catalog::ModelKind;
use crate::error::{Error, Result};

/// Bump when a change to the messages isn't backward compatible. The app
/// refuses a worker that speaks a different version and asks for a re-download.
pub const PROTOCOL_VERSION: u32 = 1;

/// Largest JSON part of a frame.
pub const MAX_JSON_BYTES: usize = 1 << 20;
/// Largest audio payload of a frame (about an hour of 16 kHz audio).
pub const MAX_PAYLOAD_BYTES: usize = 256 << 20;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    /// First message: states the version the app speaks.
    Hello { protocol: u32 },
    /// Load a model. Paths are absolute and point into the model store.
    Load {
        model_id: String,
        kind: ModelKind,
        paths: BTreeMap<String, String>,
        num_threads: Option<u32>,
    },
    /// Transcribe the audio in the frame's payload (f32 LE samples).
    Transcribe { id: u64, sample_rate: u32, language: Option<String> },
    /// Free the loaded model's memory.
    Unload,
    /// Exit cleanly.
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    /// Answer to `Hello`: the worker's version and the engine it wraps.
    Ready { protocol: u32, engine: String },
    Loaded { model_id: String },
    Transcript { id: u64, text: String, language: Option<String> },
    Unloaded,
    /// A request failed. The worker keeps running unless the failure is fatal.
    Error { id: Option<u64>, message: String },
}

/// Encodes one frame.
pub fn encode_frame<T: Serialize>(msg: &T, payload: &[u8]) -> Result<Vec<u8>> {
    let json = serde_json::to_vec(msg).map_err(|e| Error::Engine(format!("encoding a message: {e}")))?;
    if json.len() > MAX_JSON_BYTES || payload.len() > MAX_PAYLOAD_BYTES {
        return Err(Error::Engine("a message is too large to send".into()));
    }
    let mut out = Vec::with_capacity(8 + json.len() + payload.len());
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&json);
    out.extend_from_slice(payload);
    Ok(out)
}

/// Splits an 8-byte header into its two lengths, refusing absurd ones.
pub fn parse_header(h: [u8; 8]) -> Result<(usize, usize)> {
    let json = u32::from_le_bytes([h[0], h[1], h[2], h[3]]) as usize;
    let payload = u32::from_le_bytes([h[4], h[5], h[6], h[7]]) as usize;
    if json == 0 || json > MAX_JSON_BYTES || payload > MAX_PAYLOAD_BYTES {
        return Err(Error::Engine(format!(
            "the message header is invalid (json {json} bytes, payload {payload} bytes)"
        )));
    }
    Ok((json, payload))
}

pub fn decode_json<T: for<'de> Deserialize<'de>>(json: &[u8]) -> Result<T> {
    serde_json::from_slice(json).map_err(|e| Error::Engine(format!("an unreadable message: {e}")))
}

pub fn write_frame<W: Write, T: Serialize>(w: &mut W, msg: &T, payload: &[u8]) -> Result<()> {
    let frame = encode_frame(msg, payload)?;
    w.write_all(&frame).map_err(|e| Error::Engine(format!("writing a message: {e}")))?;
    w.flush().map_err(|e| Error::Engine(format!("writing a message: {e}")))
}

/// Reads one frame. `Ok(None)` means the stream ended cleanly between frames.
pub fn read_frame<R: Read>(r: &mut R) -> Result<Option<(Vec<u8>, Vec<u8>)>> {
    let mut header = [0u8; 8];
    // Distinguish "closed between frames" (fine) from "closed mid-header".
    let mut got = 0;
    while got < header.len() {
        let n = r.read(&mut header[got..]).map_err(|e| Error::Engine(format!("reading a message: {e}")))?;
        if n == 0 {
            return if got == 0 {
                Ok(None)
            } else {
                Err(Error::Engine("the stream ended in the middle of a message".into()))
            };
        }
        got += n;
    }
    let (json_len, payload_len) = parse_header(header)?;
    let mut json = vec![0u8; json_len];
    r.read_exact(&mut json).map_err(|e| Error::Engine(format!("reading a message: {e}")))?;
    let mut payload = vec![0u8; payload_len];
    r.read_exact(&mut payload).map_err(|e| Error::Engine(format!("reading a message: {e}")))?;
    Ok(Some((json, payload)))
}

pub fn samples_to_bytes(samples: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * 4);
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

pub fn bytes_to_samples(bytes: &[u8]) -> Result<Vec<f32>> {
    if bytes.len() % 4 != 0 {
        return Err(Error::BadAudio("the audio payload isn't a whole number of samples".into()));
    }
    Ok(bytes.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn requests_and_responses_round_trip_through_a_frame() {
        let req = Request::Load {
            model_id: "whisper-small".into(),
            kind: ModelKind::Whisper,
            paths: BTreeMap::from([("encoder".to_string(), "/m/enc.onnx".to_string())]),
            num_threads: Some(4),
        };
        let frame = encode_frame(&req, &[]).unwrap();
        let (json, payload) = read_frame(&mut Cursor::new(frame)).unwrap().unwrap();
        assert!(payload.is_empty());
        assert_eq!(decode_json::<Request>(&json).unwrap(), req);

        let resp = Response::Transcript { id: 7, text: "नमस्ते duniya".into(), language: Some("hi".into()) };
        let frame = encode_frame(&resp, &[]).unwrap();
        let (json, _) = read_frame(&mut Cursor::new(frame)).unwrap().unwrap();
        assert_eq!(decode_json::<Response>(&json).unwrap(), resp);
    }

    #[test]
    fn audio_travels_as_raw_bytes_and_comes_back_exactly() {
        let samples = vec![0.0f32, 0.5, -0.5, 1.0, -1.0, 0.123456];
        let req = Request::Transcribe { id: 1, sample_rate: 16_000, language: None };
        let frame = encode_frame(&req, &samples_to_bytes(&samples)).unwrap();
        // 8-byte header + JSON + 4 bytes per sample, nothing more.
        let json_len = u32::from_le_bytes([frame[0], frame[1], frame[2], frame[3]]) as usize;
        assert_eq!(frame.len(), 8 + json_len + samples.len() * 4);
        let (_, payload) = read_frame(&mut Cursor::new(frame)).unwrap().unwrap();
        assert_eq!(bytes_to_samples(&payload).unwrap(), samples);
        assert!(bytes_to_samples(&[1, 2, 3]).is_err());
    }

    #[test]
    fn several_frames_are_read_in_order_and_the_end_is_clean() {
        let mut stream = Vec::new();
        for i in 0..3u64 {
            stream.extend(encode_frame(&Response::Transcript { id: i, text: format!("t{i}"), language: None }, &[]).unwrap());
        }
        let mut r = Cursor::new(stream);
        for i in 0..3u64 {
            let (json, _) = read_frame(&mut r).unwrap().unwrap();
            match decode_json::<Response>(&json).unwrap() {
                Response::Transcript { id, .. } => assert_eq!(id, i),
                other => panic!("unexpected {other:?}"),
            }
        }
        assert!(read_frame(&mut r).unwrap().is_none(), "clean end between frames");
    }

    #[test]
    fn a_truncated_stream_is_an_error_not_a_hang_or_a_clean_end() {
        let frame = encode_frame(&Request::Unload, &[]).unwrap();
        for cut in [3, 8, frame.len() - 1] {
            assert!(read_frame(&mut Cursor::new(frame[..cut].to_vec())).is_err(), "cut at {cut}");
        }
    }

    #[test]
    fn absurd_headers_are_refused_before_allocating() {
        let mut h = [0u8; 8];
        h[..4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse_header(h).is_err());
        let mut h = [0u8; 8];
        h[..4].copy_from_slice(&10u32.to_le_bytes());
        h[4..].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse_header(h).is_err());
        assert!(parse_header([0u8; 8]).is_err(), "an empty JSON part is invalid");
        assert!(read_frame(&mut Cursor::new(u32::MAX.to_le_bytes().iter().chain(&[0u8; 4]).copied().collect::<Vec<u8>>())).is_err());
    }

    #[test]
    fn garbage_json_is_an_error() {
        assert!(decode_json::<Request>(b"{ not json").is_err());
        assert!(decode_json::<Request>(br#"{"type":"nonsense"}"#).is_err());
    }

    #[test]
    fn oversized_messages_cannot_be_encoded() {
        let big = vec![0u8; MAX_PAYLOAD_BYTES + 1];
        assert!(encode_frame(&Request::Unload, &big).is_err());
    }
}
