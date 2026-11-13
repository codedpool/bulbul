// SPDX-License-Identifier: GPL-3.0-only
// Copyright (c) 2026 Romanch Roshan Singh

//! A small local HTTP server for tests: supports `Range`, and can drop the
//! connection mid-file or ignore ranges, so the downloader's resume and retry
//! paths can be exercised for real.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[derive(Clone, Default)]
pub struct Behaviour {
    /// Answer 200 with the whole file even when a range was asked for.
    pub ignore_range: bool,
    /// On the very first request, send this many body bytes and hang up.
    pub drop_first_after: Option<usize>,
}

pub struct Server {
    pub port: u16,
    /// (path, range start) for every request received.
    pub requests: Arc<Mutex<Vec<(String, Option<u64>)>>>,
}

impl Server {
    pub fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

pub async fn serve(files: HashMap<String, Vec<u8>>, b: Behaviour) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let requests: Arc<Mutex<Vec<(String, Option<u64>)>>> = Arc::default();
    let reqs = requests.clone();
    tokio::spawn(async move {
        let mut first = true;
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { return };
            let files = files.clone();
            let b = b.clone();
            let reqs = reqs.clone();
            let drop_this = std::mem::replace(&mut first, false) && b.drop_first_after.is_some();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = sock.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                }
                let head = String::from_utf8_lossy(&buf).to_string();
                let path = head.split_whitespace().nth(1).unwrap_or("/").to_string();
                let range = head.lines().find_map(|l| {
                    let l = l.to_ascii_lowercase();
                    l.strip_prefix("range: bytes=")
                        .and_then(|v| v.trim_end_matches('-').trim().parse::<u64>().ok())
                });
                reqs.lock().unwrap().push((path.clone(), range));
                let Some(data) = files.get(&path) else {
                    let _ = sock
                        .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\nConnection: close\r\n\r\nnot found")
                        .await;
                    return;
                };
                let (status, body, extra) = match range {
                    Some(start) if !b.ignore_range && (start as usize) < data.len() => (
                        "206 Partial Content",
                        &data[start as usize..],
                        format!("Content-Range: bytes {start}-{}/{}\r\n", data.len() - 1, data.len()),
                    ),
                    _ => ("200 OK", &data[..], String::new()),
                };
                let header = format!(
                    "HTTP/1.1 {status}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = sock.write_all(header.as_bytes()).await;
                if drop_this {
                    let k = b.drop_first_after.unwrap().min(body.len());
                    let _ = sock.write_all(&body[..k]).await;
                    return; // hang up mid-body
                }
                for piece in body.chunks(16 * 1024) {
                    if sock.write_all(piece).await.is_err() {
                        return;
                    }
                }
                let _ = sock.shutdown().await;
            });
        }
    });
    Server { port, requests }
}
