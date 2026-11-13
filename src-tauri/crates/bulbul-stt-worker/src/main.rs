// SPDX-License-Identifier: GPL-3.0-only
// Copyright (c) 2026 Romanch Roshan Singh

//! Bulbul's on-device speech worker.
//!
//! It reads framed requests on stdin and answers on stdout (see
//! `bulbul_local_stt::protocol`). Logging goes to stderr only: stdout belongs
//! to the protocol. It never opens the network, reads settings, or sees an API
//! key; it is handed model file paths and audio, and returns text.
//!
//! When the app goes away, stdin closes and the worker exits.

mod engine;

use bulbul_local_stt::protocol::PROTOCOL_VERSION;

const ENGINE: &str = "sherpa-onnx 2.0.1";

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--version") {
        println!("bulbul-stt-worker {} protocol {PROTOCOL_VERSION} {ENGINE}", env!("CARGO_PKG_VERSION"));
        return;
    }

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut transcriber = engine::SherpaTranscriber::new();
    let code = match bulbul_local_stt::worker::serve(stdin.lock(), stdout.lock(), &mut transcriber, ENGINE) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("bulbul-stt-worker: {e}");
            1
        }
    };
    std::process::exit(code);
}
