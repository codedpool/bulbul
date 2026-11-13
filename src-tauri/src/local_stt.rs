// SPDX-License-Identifier: GPL-3.0-only
// Copyright (c) 2026 Romanch Roshan Singh

//! On-device transcription for the desktop app.
//!
//! Glue between the model library (`local_models.rs`), the `bulbul-local-stt`
//! crate's worker supervisor, and the rest of Bulbul: it installs the speech
//! engine, and turns a recording into text with the selected local model.
//!
//! Nothing here runs a speech model itself: that happens in a separate worker
//! program, so a crash in the engine can't take Bulbul down.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use anyhow::{anyhow, Context, Result};
use bulbul_local_stt::engine::{self, EngineManifest, ENGINE_ID};
use bulbul_local_stt::ModelKind;
use bulbul_local_stt::{
    Audio, CancelToken, Error as SttError, LoadSpec, ProcessLauncher, ProgressFn, Status, Supervisor,
    SupervisorConfig,
};
use tauri::AppHandle;

use crate::config::{self, Config};
use crate::local_models::{EngineStatus, LicenseInfo, LocalModelInfo, ModelManager, Tracker};

/// Developer override for where the worker program lives.
const WORKER_ENV: &str = "BULBUL_STT_WORKER";
/// Developer/test override for where models and the engine are kept.
const ROOT_ENV: &str = "BULBUL_LOCAL_STT_DIR";

/// Where the signed engine manifest is published (the signature sits beside it).
const MANIFEST_URL: &str = "https://github.com/codedpool/bulbul/releases/download/stt-engine/stt-engine-manifest.json";
const MANIFEST_SIG_URL: &str = "https://github.com/codedpool/bulbul/releases/download/stt-engine/stt-engine-manifest.json.sig";
/// The key that signs the manifest: the same one that signs app updates.
const SIGNING_PUBKEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDRCMEQ1M0FFRUNEQkJEQ0IKUldUTHZkdnNybE1OUzRMUXZzS08wM1Q4a0YrNWpaMXM3S2l5VTRsS1ptWVBjZDArMXF4bTJnS3QK";
/// Manifests are tiny; anything bigger is not what we expect.
const MAX_MANIFEST_BYTES: usize = 256 * 1024;

pub struct LocalStt {
    models: Arc<ModelManager>,
    engine_dir: PathBuf,
    supervisor: Arc<Supervisor<ProcessLauncher>>,
    worker_path: PathBuf,
}

static INSTANCE: OnceLock<Arc<LocalStt>> = OnceLock::new();

/// The shared instance, created on first use. Call from inside the async
/// runtime (the idle-unload timer needs it).
pub fn get() -> Result<&'static Arc<LocalStt>> {
    if let Some(i) = INSTANCE.get() {
        return Ok(i);
    }
    let created = Arc::new(LocalStt::create()?);
    Ok(INSTANCE.get_or_init(|| created))
}

/// Counts only: a catalog model id and a failure class. Never a path, a
/// transcript or anything the user typed (custom models are not reported).
fn tracker(enabled: bool) -> Option<Tracker> {
    enabled.then(|| {
        let t: Tracker = Arc::new(|event, props| crate::telemetry::track(event, props));
        t
    })
}

impl LocalStt {
    fn create() -> Result<LocalStt> {
        let root = match std::env::var_os(ROOT_ENV) {
            Some(p) => PathBuf::from(p),
            None => config::config_dir()?.join("local-stt"),
        };
        let worker_path = match std::env::var_os(WORKER_ENV) {
            Some(p) => PathBuf::from(p),
            None => root.join("engine").join(engine::worker_file_name()),
        };
        let supervisor = Supervisor::new(ProcessLauncher::new(&worker_path), SupervisorConfig::default());
        // Frees the model's memory after a quiet spell. Skipped outside the
        // runtime (tests); the next dictation just starts a fresh worker.
        if tokio::runtime::Handle::try_current().is_ok() {
            let _ = supervisor.spawn_idle_reaper();
        }
        Ok(LocalStt {
            models: ModelManager::new(root.join("models"))?,
            engine_dir: root.join("engine"),
            supervisor,
            worker_path,
        })
    }

    pub fn engine_installed(&self) -> bool {
        self.worker_path.is_file()
    }

    pub fn engine_status(&self) -> EngineStatus {
        EngineStatus { installed: self.engine_installed() }
    }

    pub fn list(&self, cfg: &Config) -> Result<Vec<LocalModelInfo>> {
        let mut out = self.models.list(None)?;
        for c in &cfg.local_custom_models {
            let ok = custom_spec(c).is_ok();
            let family = parse_kind(&c.kind).map(bulbul_local_stt::custom::kind_label).unwrap_or("unknown");
            out.push(LocalModelInfo {
                id: c.id.clone(),
                name: c.name.clone(),
                summary: format!("Your own {family} model."),
                languages: Vec::new(),
                multilingual: true,
                experimental: false,
                license: LicenseInfo { spdx: String::new(), author: String::new(), url: String::new(), attribution: None, converted: false },
                download_bytes: 0,
                disk_bytes: folder_size(std::path::Path::new(&c.folder)),
                // A moved or emptied folder reads as not installed, so the
                // card says so instead of failing at dictation time.
                status: if ok { "installed" } else { "not_installed" },
                verified: false,
                partial_bytes: 0,
                downloading: false,
                custom: true,
                folder: Some(c.folder.clone()),
            });
        }
        Ok(out)
    }

    pub fn start_download(self: &Arc<Self>, app: AppHandle, id: &str, telemetry: bool) -> Result<()> {
        self.models.start_download(app, id, tracker(telemetry))
    }

    /// Downloads and installs the speech engine in the background, reporting
    /// through the same events as a model (under the id `engine`).
    pub fn start_engine_install(self: &Arc<Self>, app: AppHandle, telemetry: bool) -> Result<()> {
        let this = self.clone();
        self.models.spawn_job(
            app,
            ENGINE_ID,
            "The speech engine is already being installed".to_string(),
            tracker(telemetry),
            move |token, cb| async move { this.install_engine(&token, cb).await },
        )
    }

    async fn install_engine(&self, cancel: &CancelToken, on_progress: ProgressFn) -> std::result::Result<(), SttError> {
        // A running worker holds its own file open (Windows won't let us replace it).
        self.supervisor.unload().await;
        let manifest = fetch_manifest().await?;
        self.models.installer().install_engine(&manifest, &self.engine_dir, cancel, on_progress).await?;
        Ok(())
    }

    pub fn cancel_download(&self, id: &str) {
        self.models.cancel(id);
    }

    /// Removes a model's files. Also stops the worker if it has this model
    /// loaded, so nothing is left holding the files open.
    pub async fn delete(&self, id: &str) -> Result<()> {
        self.models.catalog().get(id).map_err(|e| anyhow!("{e}"))?;
        if self.supervisor.loaded_model().await.as_deref() == Some(id) {
            self.supervisor.unload().await;
        }
        self.models.delete(id)
    }

    /// Transcribes a WAV recording with the model selected in `cfg`.
    pub async fn transcribe(&self, cfg: &Config, wav: &[u8], language: Option<&str>) -> Result<String> {
        let (name, spec) = self.spec_for(cfg)?;
        if !self.engine_installed() {
            return Err(anyhow!("The on-device speech engine isn't installed yet."));
        }
        self.supervisor.set_idle_unload(idle_unload(cfg));
        let audio = Audio::from_wav_bytes(wav).map_err(|e| anyhow!("{e}"))?;
        let out = self
            .supervisor
            .transcribe(&spec, &audio, language)
            .await
            .map_err(|e| anyhow!("{e}"))
            .with_context(|| format!("transcribing with {name}"))?;
        Ok(clean_output(&out.text))
    }

    /// The model selected in `cfg`, as something the worker can load.
    fn spec_for(&self, cfg: &Config) -> Result<(String, LoadSpec)> {
        let id = cfg.local_stt_model.trim();
        if let Some(c) = cfg.local_custom_models.iter().find(|c| c.id == id) {
            let spec = custom_spec(c).map_err(|e| anyhow!("{} can't be loaded: {e}", c.name))?;
            return Ok((c.name.clone(), spec));
        }
        let entry = self.models.catalog().get(id).map_err(|_| anyhow!("No on-device model is selected."))?;
        match self.models.store().status(entry).map_err(|e| anyhow!("{e}"))? {
            Status::Installed { .. } => {}
            _ => return Err(anyhow!("{} isn't downloaded yet.", entry.name)),
        }
        let spec = LoadSpec::from_entry(self.models.store(), entry, None).map_err(|e| anyhow!("{e}"))?;
        Ok((entry.name.clone(), spec))
    }

    /// Whether the selected model is already in memory, so the next
    /// dictation won't wait for it to load.
    pub async fn is_loaded(&self, cfg: &Config) -> bool {
        self.supervisor.loaded_model().await.as_deref() == Some(cfg.local_stt_model.trim())
    }
}

fn parse_kind(kind: &str) -> Option<ModelKind> {
    serde_json::from_value(serde_json::Value::String(kind.to_string())).ok()
}

fn custom_spec(c: &config::CustomLocalModel) -> std::result::Result<LoadSpec, SttError> {
    let kind = parse_kind(&c.kind).ok_or_else(|| SttError::Engine(format!("unknown model family {}", c.kind)))?;
    LoadSpec::from_folder(&c.id, kind, std::path::Path::new(&c.folder), None)
}

/// Checks that `folder` holds a model of `kind` Bulbul can load.
pub fn check_custom(kind: &str, folder: &str) -> Result<()> {
    let kind = parse_kind(kind).ok_or_else(|| anyhow!("Pick the model family."))?;
    let dir = std::path::Path::new(folder.trim());
    if !dir.is_dir() {
        return Err(anyhow!("That folder doesn't exist."));
    }
    bulbul_local_stt::custom::detect_layout(kind, dir).map(|_| ()).map_err(|e| anyhow!("{e}"))
}

fn folder_size(dir: &std::path::Path) -> u64 {
    let Ok(read) = std::fs::read_dir(dir) else { return 0 };
    read.flatten()
        .map(|e| match e.metadata() {
            Ok(m) if m.is_dir() => folder_size(&e.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        })
        .sum()
}

fn idle_unload(cfg: &Config) -> std::time::Duration {
    std::time::Duration::from_secs(u64::from(cfg.local_idle_unload_minutes) * 60)
}

/// Tidies a model's raw output so it reads like the cloud path's: one line,
/// single spaces, no stray edges. Wording is never changed.
fn clean_output(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Fetches the engine manifest and refuses it unless its signature checks out.
async fn fetch_manifest() -> std::result::Result<EngineManifest, SttError> {
    let net = |e: reqwest::Error| SttError::Network { what: "the engine manifest".into(), message: e.to_string() };
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("Bulbul")
        .build()
        .map_err(net)?;
    let get = |url: &'static str| {
        let client = client.clone();
        async move {
            let resp = client.get(url).send().await.map_err(net)?.error_for_status().map_err(net)?;
            let bytes = resp.bytes().await.map_err(net)?;
            if bytes.len() > MAX_MANIFEST_BYTES {
                return Err(SttError::Engine("the engine manifest is unexpectedly large".into()));
            }
            Ok(bytes)
        }
    };
    let manifest = get(MANIFEST_URL).await?;
    let sig = String::from_utf8_lossy(&get(MANIFEST_SIG_URL).await?).into_owned();
    engine::verify_manifest(&manifest, &sig, SIGNING_PUBKEY)?;
    EngineManifest::parse(&manifest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_manifest_key_is_the_one_that_signs_app_updates() {
        let conf: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(conf["plugins"]["updater"]["pubkey"].as_str(), Some(SIGNING_PUBKEY));
    }

    #[test]
    fn output_is_tidied_without_changing_words() {
        assert_eq!(clean_output("  hello\n  world \t again  "), "hello world again");
        assert_eq!(clean_output(""), "");
    }

    #[test]
    fn custom_models_are_checked_before_they_are_saved() {
        assert!(check_custom("nonsense", "/tmp").is_err());
        assert!(check_custom("whisper", "/definitely/not/here").unwrap_err().to_string().contains("doesn't exist"));
        let d = tempfile::tempdir().unwrap();
        assert!(check_custom("whisper", d.path().to_str().unwrap()).unwrap_err().to_string().contains("missing"));
        for f in ["model.onnx", "tokens.txt"] {
            std::fs::write(d.path().join(f), b"x").unwrap();
        }
        check_custom("omnilingual_ctc", d.path().to_str().unwrap()).unwrap();
        assert!(folder_size(d.path()) >= 2);
    }

    #[test]
    fn the_idle_setting_converts_minutes() {
        let mut c = Config::default();
        c.local_idle_unload_minutes = 0;
        assert!(idle_unload(&c).is_zero());
        c.local_idle_unload_minutes = 7;
        assert_eq!(idle_unload(&c).as_secs(), 420);
    }

    #[test]
    fn manifest_urls_are_https() {
        assert!(MANIFEST_URL.starts_with("https://") && MANIFEST_SIG_URL.starts_with("https://"));
    }

    #[test]
    fn the_worker_file_name_matches_the_platform() {
        assert_eq!(engine::worker_file_name().ends_with(".exe"), cfg!(windows));
    }
}
