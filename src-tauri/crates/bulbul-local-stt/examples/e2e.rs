// SPDX-License-Identifier: GPL-3.0-only
// Copyright (c) 2026 Romanch Roshan Singh

//! Developer tool: installs a catalog model, starts the real worker program
//! and transcribes a WAV file with it, printing timings.
//!
//! ```text
//! cargo run --release --example e2e -- <worker-exe> <store-dir> <model-id> <file.wav> [language]
//! ```

use std::sync::Arc;
use std::time::Instant;

use bulbul_local_stt::{
    Audio, CancelToken, Catalog, Installer, LoadSpec, Phase, ProcessLauncher, Progress, Status, Store,
    Supervisor, SupervisorConfig,
};

#[tokio::main]
async fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 5 {
        eprintln!("usage: e2e <worker-exe> <store-dir> <model-id> <file.wav> [language]");
        std::process::exit(2);
    }
    let (worker, store_dir, model_id, wav_path) = (&a[1], &a[2], &a[3], &a[4]);
    let language = a.get(5).map(String::as_str);

    let catalog = Catalog::bundled().expect("bundled catalog");
    let entry = catalog.get(model_id).expect("model id").clone();
    let store = Store::new(store_dir);

    if store.status(&entry).unwrap() != (Status::Installed { verified: true }) {
        let installer = Installer::new(store.clone()).expect("installer");
        let last = Arc::new(std::sync::Mutex::new((Instant::now(), 0u64)));
        let l = last.clone();
        let cb: bulbul_local_stt::ProgressFn = Arc::new(move |p: Progress| {
            let mut l = l.lock().unwrap();
            if p.phase != Phase::Downloading || l.0.elapsed().as_millis() > 1500 || p.bytes_done == p.bytes_total {
                eprintln!("  {:?} {}/{} MB", p.phase, p.bytes_done / 1_000_000, p.bytes_total / 1_000_000);
                *l = (Instant::now(), p.bytes_done);
            }
        });
        let t = Instant::now();
        installer.install(&entry, &CancelToken::new(), cb).await.expect("install");
        eprintln!("installed {} in {:.1}s", entry.id, t.elapsed().as_secs_f32());
    } else {
        eprintln!("{} already installed", entry.id);
    }

    let audio = Audio::from_wav_bytes(&std::fs::read(wav_path).expect("read wav")).expect("parse wav");
    eprintln!("audio: {:.1}s @ {} Hz", audio.duration_ms() as f32 / 1000.0, audio.sample_rate);

    let spec = LoadSpec::from_entry(&store, &entry, None).expect("spec");
    let sup = Supervisor::new(ProcessLauncher::new(worker), SupervisorConfig::default());

    for run in ["cold (start worker + load model)", "warm"] {
        let t = Instant::now();
        match sup.transcribe(&spec, &audio, language).await {
            Ok(out) => {
                let secs = t.elapsed().as_secs_f32();
                println!("[{run}] {secs:.2}s  (RTF {:.2})", secs / (audio.duration_ms() as f32 / 1000.0));
                println!("{}", out.text);
            }
            Err(e) => {
                println!("[{run}] FAILED: {e}");
                std::process::exit(1);
            }
        }
    }
    sup.unload().await;
}
