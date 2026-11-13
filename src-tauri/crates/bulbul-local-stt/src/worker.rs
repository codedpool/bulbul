// SPDX-License-Identifier: GPL-3.0-only
// Copyright (c) 2026 Romanch Roshan Singh

//! The worker program's main loop, written against the [`Transcriber`] trait.
//!
//! The real worker plugs the speech engine in here; the tests plug in a fake,
//! so the whole conversation (handshake, load, transcribe, errors, shutdown)
//! is exercised without a model.

use std::io::{Read, Write};

use crate::error::{Error, Result};
use crate::protocol::{
    bytes_to_samples, decode_json, read_frame, write_frame, Request, Response, PROTOCOL_VERSION,
};
use crate::transcriber::{Audio, LoadSpec, TranscribeOptions, Transcriber};

/// Serves requests from `input` until the app says `Shutdown` or closes the
/// stream (which is also how the worker learns the app has gone away).
///
/// A failed request gets an `Error` response and the loop carries on. Only a
/// broken stream or a version mismatch ends it with an error.
pub fn serve<R: Read, W: Write, T: Transcriber>(
    mut input: R,
    mut output: W,
    transcriber: &mut T,
    engine: &str,
) -> Result<()> {
    let mut greeted = false;
    loop {
        let Some((json, payload)) = read_frame(&mut input)? else { return Ok(()) };
        let request: Request = match decode_json(&json) {
            Ok(r) => r,
            Err(e) => {
                write_frame(&mut output, &Response::Error { id: None, message: e.to_string() }, &[])?;
                continue;
            }
        };

        // Nothing but a Hello is accepted first, so a worker never acts on a
        // request from something that hasn't agreed on the protocol.
        if !greeted && !matches!(request, Request::Hello { .. }) {
            write_frame(
                &mut output,
                &Response::Error { id: None, message: "expected a hello first".into() },
                &[],
            )?;
            continue;
        }

        match request {
            Request::Hello { protocol } => {
                if protocol != PROTOCOL_VERSION {
                    write_frame(
                        &mut output,
                        &Response::Error {
                            id: None,
                            message: format!(
                                "protocol version mismatch (app speaks {protocol}, worker speaks {PROTOCOL_VERSION})"
                            ),
                        },
                        &[],
                    )?;
                    return Err(Error::Engine("protocol version mismatch".into()));
                }
                greeted = true;
                write_frame(
                    &mut output,
                    &Response::Ready { protocol: PROTOCOL_VERSION, engine: engine.to_string() },
                    &[],
                )?;
            }
            Request::Load { model_id, kind, paths, num_threads } => {
                let spec = LoadSpec {
                    model_id: model_id.clone(),
                    kind,
                    paths: paths.into_iter().map(|(k, v)| (k, v.into())).collect(),
                    num_threads,
                };
                let reply = match transcriber.load(&spec) {
                    Ok(()) => Response::Loaded { model_id },
                    Err(e) => Response::Error { id: None, message: e.to_string() },
                };
                write_frame(&mut output, &reply, &[])?;
            }
            Request::Transcribe { id, sample_rate, language } => {
                let reply = match bytes_to_samples(&payload) {
                    Err(e) => Response::Error { id: Some(id), message: e.to_string() },
                    Ok(samples) => {
                        let opts = TranscribeOptions { language, hotwords: Vec::new() };
                        match transcriber.transcribe(&Audio { sample_rate, samples }, &opts) {
                            Ok(t) => Response::Transcript { id, text: t.text, language: t.language },
                            Err(e) => Response::Error { id: Some(id), message: e.to_string() },
                        }
                    }
                };
                write_frame(&mut output, &reply, &[])?;
            }
            Request::Unload => {
                transcriber.unload();
                write_frame(&mut output, &Response::Unloaded, &[])?;
            }
            Request::Shutdown => return Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::ModelKind;
    use crate::protocol::{encode_frame, samples_to_bytes};
    use crate::testing::FakeTranscriber;
    use std::collections::BTreeMap;
    use std::io::Cursor;

    fn req(r: &Request, payload: &[u8]) -> Vec<u8> {
        encode_frame(r, payload).unwrap()
    }

    fn load_req() -> Request {
        Request::Load {
            model_id: "m".into(),
            kind: ModelKind::Whisper,
            paths: BTreeMap::new(),
            num_threads: None,
        }
    }

    fn run(input: Vec<u8>, t: &mut FakeTranscriber) -> (Result<()>, Vec<Response>) {
        let mut out = Vec::new();
        let result = serve(Cursor::new(input), &mut out, t, "fake");
        let mut r = Cursor::new(out);
        let mut responses = Vec::new();
        while let Some((json, _)) = read_frame(&mut r).unwrap() {
            responses.push(decode_json::<Response>(&json).unwrap());
        }
        (result, responses)
    }

    #[test]
    fn a_full_conversation() {
        let mut t = FakeTranscriber::replying("hello world");
        let mut input = Vec::new();
        input.extend(req(&Request::Hello { protocol: PROTOCOL_VERSION }, &[]));
        input.extend(req(&load_req(), &[]));
        input.extend(req(
            &Request::Transcribe { id: 42, sample_rate: 16_000, language: Some("hi".into()) },
            &samples_to_bytes(&[0.0; 160]),
        ));
        input.extend(req(&Request::Unload, &[]));
        input.extend(req(&Request::Shutdown, &[]));

        let (result, out) = run(input, &mut t);
        result.unwrap();
        assert_eq!(out.len(), 4);
        assert_eq!(out[0], Response::Ready { protocol: PROTOCOL_VERSION, engine: "fake".into() });
        assert_eq!(out[1], Response::Loaded { model_id: "m".into() });
        assert_eq!(
            out[2],
            Response::Transcript { id: 42, text: "hello world".into(), language: Some("hi".into()) }
        );
        assert_eq!(out[3], Response::Unloaded);
        assert_eq!(t.last_sample_count, Some(160));
        assert_eq!(t.loaded_model(), None, "unloaded at the end");
    }

    #[test]
    fn a_version_mismatch_is_refused_and_ends_the_worker() {
        let mut t = FakeTranscriber::replying("x");
        let (result, out) = run(req(&Request::Hello { protocol: PROTOCOL_VERSION + 1 }, &[]), &mut t);
        assert!(result.is_err());
        assert!(matches!(&out[0], Response::Error { message, .. } if message.contains("mismatch")));
    }

    #[test]
    fn nothing_is_acted_on_before_the_hello() {
        let mut t = FakeTranscriber::replying("x");
        let mut input = req(&load_req(), &[]);
        input.extend(req(&Request::Hello { protocol: PROTOCOL_VERSION }, &[]));
        input.extend(req(&load_req(), &[]));
        let (result, out) = run(input, &mut t);
        result.unwrap();
        assert!(matches!(out[0], Response::Error { .. }), "load before hello is refused");
        assert!(matches!(out[1], Response::Ready { .. }));
        assert_eq!(out[2], Response::Loaded { model_id: "m".into() });
    }

    #[test]
    fn engine_errors_are_reported_and_the_worker_keeps_going() {
        let mut t = FakeTranscriber::replying("recovered");
        t.fail_next = Some("model file is corrupt".into());
        let mut input = req(&Request::Hello { protocol: PROTOCOL_VERSION }, &[]);
        input.extend(req(&load_req(), &[])); // fails once
        input.extend(req(&load_req(), &[])); // then succeeds
        input.extend(req(
            &Request::Transcribe { id: 9, sample_rate: 16_000, language: None },
            &samples_to_bytes(&[0.0; 16]),
        ));
        let (result, out) = run(input, &mut t);
        result.unwrap();
        assert!(matches!(&out[1], Response::Error { message, .. } if message.contains("corrupt")));
        assert_eq!(out[2], Response::Loaded { model_id: "m".into() });
        assert!(matches!(&out[3], Response::Transcript { id: 9, .. }));
    }

    #[test]
    fn transcribing_without_a_loaded_model_is_an_error_for_that_request() {
        let mut t = FakeTranscriber::replying("x");
        let mut input = req(&Request::Hello { protocol: PROTOCOL_VERSION }, &[]);
        input.extend(req(
            &Request::Transcribe { id: 5, sample_rate: 16_000, language: None },
            &samples_to_bytes(&[0.0; 16]),
        ));
        let (result, out) = run(input, &mut t);
        result.unwrap();
        assert!(matches!(&out[1], Response::Error { id: Some(5), .. }));
    }

    #[test]
    fn a_bad_audio_payload_is_an_error_for_that_request() {
        let mut t = FakeTranscriber::replying("x");
        let mut input = req(&Request::Hello { protocol: PROTOCOL_VERSION }, &[]);
        input.extend(req(&load_req(), &[]));
        input.extend(req(&Request::Transcribe { id: 6, sample_rate: 16_000, language: None }, &[1, 2, 3]));
        let (result, out) = run(input, &mut t);
        result.unwrap();
        assert!(matches!(&out[2], Response::Error { id: Some(6), .. }));
    }

    #[test]
    fn closing_the_stream_ends_the_worker_cleanly() {
        let mut t = FakeTranscriber::replying("x");
        let (result, out) = run(req(&Request::Hello { protocol: PROTOCOL_VERSION }, &[]), &mut t);
        result.unwrap();
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn a_garbled_message_is_reported_without_ending_the_worker() {
        let mut t = FakeTranscriber::replying("x");
        let mut input = Vec::new();
        // A well-formed frame whose JSON is nonsense.
        let junk = b"{ nope";
        input.extend((junk.len() as u32).to_le_bytes());
        input.extend(0u32.to_le_bytes());
        input.extend(junk);
        input.extend(req(&Request::Hello { protocol: PROTOCOL_VERSION }, &[]));
        let (result, out) = run(input, &mut t);
        result.unwrap();
        assert!(matches!(out[0], Response::Error { .. }));
        assert!(matches!(out[1], Response::Ready { .. }));
    }
}
