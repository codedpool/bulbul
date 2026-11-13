// SPDX-License-Identifier: GPL-3.0-only
// Copyright (c) 2026 Romanch Roshan Singh

//! The app's side of the conversation with the worker program, over any pair
//! of async streams (the worker's stdin/stdout in production, in-memory pipes
//! in tests).

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::error::{Error, Result};
use crate::protocol::{
    decode_json, encode_frame, parse_header, samples_to_bytes, Request, Response, PROTOCOL_VERSION,
};
use crate::transcriber::{Audio, LoadSpec, Transcript};

pub struct WorkerClient<R, W> {
    reader: R,
    writer: W,
    next_id: u64,
    /// What the worker said it wraps, e.g. `sherpa-onnx 2.0.1`.
    pub engine: String,
}

impl<R: AsyncRead + Unpin, W: AsyncWrite + Unpin> WorkerClient<R, W> {
    /// Says hello and checks the worker speaks our protocol version.
    pub async fn handshake(reader: R, writer: W) -> Result<Self> {
        let mut c = WorkerClient { reader, writer, next_id: 1, engine: String::new() };
        c.send(&Request::Hello { protocol: PROTOCOL_VERSION }, &[]).await?;
        match c.recv().await? {
            Response::Ready { protocol, engine } if protocol == PROTOCOL_VERSION => {
                c.engine = engine;
                Ok(c)
            }
            Response::Ready { protocol, .. } => Err(Error::Engine(format!(
                "the speech worker speaks protocol {protocol}, this app speaks {PROTOCOL_VERSION}; it needs to be re-downloaded"
            ))),
            Response::Error { message, .. } => Err(Error::Engine(message)),
            other => Err(Error::Engine(format!("unexpected answer to hello: {other:?}"))),
        }
    }

    pub async fn load(&mut self, spec: &LoadSpec) -> Result<()> {
        let paths = spec
            .paths
            .iter()
            .map(|(k, v)| (k.clone(), v.to_string_lossy().into_owned()))
            .collect();
        self.send(
            &Request::Load { model_id: spec.model_id.clone(), kind: spec.kind, paths, num_threads: spec.num_threads },
            &[],
        )
        .await?;
        match self.recv().await? {
            Response::Loaded { .. } => Ok(()),
            Response::Error { message, .. } => Err(Error::Engine(message)),
            other => Err(Error::Engine(format!("unexpected answer to load: {other:?}"))),
        }
    }

    pub async fn transcribe(&mut self, audio: &Audio, language: Option<&str>) -> Result<Transcript> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(
            &Request::Transcribe { id, sample_rate: audio.sample_rate, language: language.map(str::to_string) },
            &samples_to_bytes(&audio.samples),
        )
        .await?;
        loop {
            match self.recv().await? {
                Response::Transcript { id: got, text, language } if got == id => {
                    return Ok(Transcript { text, language })
                }
                Response::Error { message, id: Some(got) } if got == id => return Err(Error::Engine(message)),
                Response::Error { message, id: None } => return Err(Error::Engine(message)),
                // A late answer to something we already gave up on.
                Response::Transcript { .. } | Response::Error { .. } => continue,
                other => return Err(Error::Engine(format!("unexpected answer to transcribe: {other:?}"))),
            }
        }
    }

    pub async fn unload(&mut self) -> Result<()> {
        self.send(&Request::Unload, &[]).await?;
        match self.recv().await? {
            Response::Unloaded => Ok(()),
            Response::Error { message, .. } => Err(Error::Engine(message)),
            other => Err(Error::Engine(format!("unexpected answer to unload: {other:?}"))),
        }
    }

    /// Asks the worker to exit. Best effort: the supervisor also kills it.
    pub async fn shutdown(mut self) {
        let _ = self.send(&Request::Shutdown, &[]).await;
    }

    async fn send(&mut self, req: &Request, payload: &[u8]) -> Result<()> {
        let frame = encode_frame(req, payload)?;
        self.writer.write_all(&frame).await.map_err(gone)?;
        self.writer.flush().await.map_err(gone)
    }

    async fn recv(&mut self) -> Result<Response> {
        let mut header = [0u8; 8];
        self.reader.read_exact(&mut header).await.map_err(gone)?;
        let (json_len, payload_len) = parse_header(header)?;
        let mut json = vec![0u8; json_len];
        self.reader.read_exact(&mut json).await.map_err(gone)?;
        if payload_len > 0 {
            let mut skip = vec![0u8; payload_len];
            self.reader.read_exact(&mut skip).await.map_err(gone)?;
        }
        decode_json(&json)
    }
}

/// A broken pipe or early end of stream means the worker is gone.
fn gone(e: std::io::Error) -> Error {
    Error::WorkerGone(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{write_frame, Request as Req};
    use tokio::io::duplex;

    /// An in-process stand-in for the worker, speaking the real protocol over
    /// in-memory pipes (the real server loop is tested in `worker.rs`).
    async fn with_fake_worker<F, Fut>(replying: &str, f: F)
    where
        F: FnOnce(WorkerClient<tokio::io::DuplexStream, tokio::io::DuplexStream>) -> Fut,
        Fut: std::future::Future<Output = ()>,
    {
        let (a_read, a_write) = duplex(1 << 20); // server -> client
        let (b_read, b_write) = duplex(1 << 20); // client -> server
        let reply = replying.to_string();
        let server = tokio::spawn(async move {
            // Tiny async stand-in for `worker::serve`, built from the same protocol.
            let mut input = b_read;
            let mut output = a_write;
            let mut loaded = false;
            loop {
                let mut header = [0u8; 8];
                if input.read_exact(&mut header).await.is_err() {
                    return;
                }
                let (jl, pl) = parse_header(header).unwrap();
                let mut json = vec![0u8; jl];
                input.read_exact(&mut json).await.unwrap();
                let mut payload = vec![0u8; pl];
                input.read_exact(&mut payload).await.unwrap();
                let resp = match decode_json::<Req>(&json).unwrap() {
                    Req::Hello { protocol } => Response::Ready { protocol, engine: "fake 1".into() },
                    Req::Load { model_id, .. } => {
                        loaded = true;
                        Response::Loaded { model_id }
                    }
                    Req::Transcribe { id, .. } if loaded => {
                        Response::Transcript { id, text: reply.clone(), language: None }
                    }
                    Req::Transcribe { id, .. } => {
                        Response::Error { id: Some(id), message: "no model loaded".into() }
                    }
                    Req::Unload => {
                        loaded = false;
                        Response::Unloaded
                    }
                    Req::Shutdown => return,
                };
                let mut buf = Vec::new();
                write_frame(&mut buf, &resp, &[]).unwrap();
                if output.write_all(&buf).await.is_err() {
                    return;
                }
            }
        });
        let client = WorkerClient::handshake(a_read, b_write).await.unwrap();
        f(client).await;
        let _ = server.await;
    }

    fn spec() -> LoadSpec {
        LoadSpec {
            model_id: "m".into(),
            kind: crate::catalog::ModelKind::Whisper,
            paths: Default::default(),
            num_threads: None,
        }
    }

    #[tokio::test]
    async fn handshake_load_transcribe_unload() {
        with_fake_worker("hello there", |mut c| async move {
            assert_eq!(c.engine, "fake 1");
            assert!(c.transcribe(&Audio { sample_rate: 16_000, samples: vec![0.0; 16] }, None).await.is_err());
            c.load(&spec()).await.unwrap();
            let t = c.transcribe(&Audio { sample_rate: 16_000, samples: vec![0.0; 16] }, Some("hi")).await.unwrap();
            assert_eq!(t.text, "hello there");
            c.unload().await.unwrap();
            c.shutdown().await;
        })
        .await;
    }

    #[tokio::test]
    async fn a_version_mismatch_is_reported_clearly() {
        let (a_read, mut a_write) = duplex(4096);
        let (_b_read, b_write) = duplex(4096);
        let mut buf = Vec::new();
        write_frame(&mut buf, &Response::Ready { protocol: PROTOCOL_VERSION + 5, engine: "old".into() }, &[]).unwrap();
        a_write.write_all(&buf).await.unwrap();
        let err = match WorkerClient::handshake(a_read, b_write).await {
            Err(e) => e.to_string(),
            Ok(_) => panic!("should have been refused"),
        };
        assert!(err.contains("re-downloaded"), "{err}");
    }

    #[tokio::test]
    async fn a_closed_pipe_is_reported_as_the_worker_being_gone() {
        let (a_read, a_write) = duplex(4096);
        let (_b_read, b_write) = duplex(4096);
        drop(a_write); // the worker "crashed" before answering
        let err = match WorkerClient::handshake(a_read, b_write).await {
            Err(e) => e,
            Ok(_) => panic!("should fail"),
        };
        assert!(matches!(err, Error::WorkerGone(_)), "{err}");
    }
}
