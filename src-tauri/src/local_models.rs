// SPDX-License-Identifier: GPL-3.0-only
// Copyright (c) 2026 Romanch Roshan Singh

//! The on-device model library, shared by desktop and Android: which models
//! exist, which are downloaded, and background downloads that report their
//! progress to the UI as events.
//!
//! Running a model is not here. Desktop does that in a separate worker
//! program (`local_stt.rs`); Android does it in its own Android process
//! (Kotlin). Both read the model files this module puts on disk.

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use bulbul_local_stt::{
    CancelToken, Catalog, Error as SttError, Installer, Phase, Progress, ProgressFn, Status, Store,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

/// Reports a counted event (never content). `None` when telemetry is off.
pub type Tracker = Arc<dyn Fn(&str, serde_json::Value) + Send + Sync>;

pub struct ModelManager {
    catalog: Catalog,
    store: Store,
    installer: Arc<Installer>,
    downloads: Mutex<HashMap<String, CancelToken>>,
}

// ---------- what the UI sees ----------

#[derive(Serialize)]
pub struct LicenseInfo {
    pub spdx: String,
    pub author: String,
    pub url: String,
    /// The credit line to show next to the model, when the licence requires one.
    pub attribution: Option<String>,
    pub converted: bool,
}

#[derive(Serialize)]
pub struct LocalModelInfo {
    pub id: String,
    pub name: String,
    pub summary: String,
    pub languages: Vec<String>,
    pub multilingual: bool,
    pub experimental: bool,
    pub license: LicenseInfo,
    pub download_bytes: u64,
    pub disk_bytes: u64,
    /// `not_installed` | `incomplete` | `installed`
    pub status: &'static str,
    /// For `installed`: false when the folder was filled by hand.
    pub verified: bool,
    /// For `incomplete`: how much is already on disk (so a resume is cheap).
    pub partial_bytes: u64,
    pub downloading: bool,
    /// A model the user pointed Bulbul at, not one from the catalog.
    pub custom: bool,
    /// For custom models: the folder it lives in (shown, never uploaded).
    pub folder: Option<String>,
}

#[derive(Serialize)]
pub struct EngineStatus {
    pub installed: bool,
}

#[derive(Serialize, Clone)]
pub struct ProgressEvent {
    pub model_id: String,
    /// `downloading` | `verifying` | `unpacking` | `done`
    pub phase: &'static str,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

#[derive(Serialize, Clone)]
pub struct ErrorEvent {
    pub model_id: String,
    /// `cancelled` | `no_space` | `network` | `corrupt` | `other`
    pub kind: &'static str,
    pub message: String,
    pub needed_bytes: Option<u64>,
    pub available_bytes: Option<u64>,
}

pub fn phase_name(p: &Phase) -> &'static str {
    match p {
        Phase::Downloading => "downloading",
        Phase::Verifying => "verifying",
        Phase::Unpacking => "unpacking",
        Phase::Done => "done",
    }
}

pub fn error_event(model_id: &str, e: &SttError) -> ErrorEvent {
    let (kind, needed, available) = match e {
        SttError::Cancelled => ("cancelled", None, None),
        SttError::NotEnoughSpace { needed, available } => ("no_space", Some(*needed), Some(*available)),
        SttError::Network { .. } => ("network", None, None),
        SttError::HashMismatch { .. } | SttError::Archive(_) => ("corrupt", None, None),
        _ => ("other", None, None),
    };
    ErrorEvent { model_id: model_id.to_string(), kind, message: e.to_string(), needed_bytes: needed, available_bytes: available }
}

impl ModelManager {
    /// `models_dir` is where each model gets its own folder.
    pub fn new(models_dir: PathBuf) -> Result<Arc<ModelManager>> {
        let store = Store::new(models_dir);
        let installer = Installer::new(store.clone()).map_err(|e| anyhow!("{e}"))?;
        Ok(Arc::new(ModelManager {
            catalog: Catalog::bundled().map_err(|e| anyhow!("{e}"))?,
            store,
            installer: Arc::new(installer),
            downloads: Mutex::new(HashMap::new()),
        }))
    }

    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn installer(&self) -> &Arc<Installer> {
        &self.installer
    }

    /// The catalog's models with their state on disk. `only` limits the list
    /// to those ids (Android offers a subset).
    pub fn list(&self, only: Option<&[&str]>) -> Result<Vec<LocalModelInfo>> {
        let downloading = self.downloads.lock().unwrap_or_else(|e| e.into_inner());
        let mut out = Vec::new();
        for m in &self.catalog.models {
            if only.is_some_and(|ids| !ids.contains(&m.id.as_str())) {
                continue;
            }
            let status = self.store.status(m).map_err(|e| anyhow!("{e}"))?;
            let (status_name, verified) = match status {
                Status::NotInstalled => ("not_installed", false),
                Status::Incomplete => ("incomplete", false),
                Status::Installed { verified } => ("installed", verified),
            };
            out.push(LocalModelInfo {
                id: m.id.clone(),
                name: m.name.clone(),
                summary: m.summary.clone(),
                languages: m.languages.clone(),
                multilingual: m.multilingual,
                experimental: m.experimental,
                license: LicenseInfo {
                    spdx: m.license.spdx.clone(),
                    author: m.license.author.clone(),
                    url: m.license.url.clone(),
                    attribution: m.license.attribution.clone(),
                    converted: m.license.converted,
                },
                download_bytes: m.download_bytes,
                disk_bytes: m.disk_bytes,
                status: status_name,
                verified,
                partial_bytes: if status == Status::Incomplete { self.store.disk_usage(&m.id).unwrap_or(0) } else { 0 },
                downloading: downloading.contains_key(&m.id),
                custom: false,
                folder: None,
            });
        }
        Ok(out)
    }

    /// Starts downloading `id` in the background. Progress and the outcome
    /// arrive as `local-model-progress`, `local-model-done` and
    /// `local-model-error` events.
    pub fn start_download(self: &Arc<Self>, app: AppHandle, id: &str, tracker: Option<Tracker>) -> Result<()> {
        let entry = self.catalog.get(id).map_err(|e| anyhow!("{e}"))?.clone();
        let installer = self.installer.clone();
        let busy = format!("{} is already downloading", entry.name);
        self.spawn_job(app, id, busy, tracker, move |token, cb| async move {
            installer.install(&entry, &token, cb).await
        })
    }

    /// Runs a cancellable background job under `job_id`, reporting progress and
    /// the outcome through the same events as a model download. Used for models
    /// and, on desktop, for the speech engine.
    pub fn spawn_job<F, Fut>(
        self: &Arc<Self>,
        app: AppHandle,
        job_id: &str,
        busy_message: String,
        tracker: Option<Tracker>,
        run: F,
    ) -> Result<()>
    where
        F: FnOnce(CancelToken, ProgressFn) -> Fut + Send + 'static,
        Fut: Future<Output = std::result::Result<(), SttError>> + Send + 'static,
    {
        let token = CancelToken::new();
        {
            let mut d = self.downloads.lock().unwrap_or_else(|e| e.into_inner());
            if d.contains_key(job_id) {
                return Err(anyhow!(busy_message));
            }
            d.insert(job_id.to_string(), token.clone());
        }
        let this = self.clone();
        let id = job_id.to_string();
        tauri::async_runtime::spawn(async move {
            let emitter = app.clone();
            let cb: ProgressFn = Arc::new(move |p: Progress| {
                let _ = emitter.emit(
                    "local-model-progress",
                    ProgressEvent {
                        model_id: p.model_id,
                        phase: phase_name(&p.phase),
                        bytes_done: p.bytes_done,
                        bytes_total: p.bytes_total,
                    },
                );
            });
            let result = run(token, cb).await;
            this.downloads.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
            match result {
                Ok(()) => {
                    if let Some(t) = &tracker {
                        t("local_model_installed", serde_json::json!({ "model": id }));
                    }
                    let _ = app.emit("local-model-done", id.clone());
                }
                Err(e) => {
                    tracing::warn!("local model job {id} failed: {e}");
                    let event = error_event(&id, &e);
                    if event.kind != "cancelled" {
                        if let Some(t) = &tracker {
                            t("local_model_failed", serde_json::json!({ "model": id, "kind": event.kind }));
                        }
                    }
                    let _ = app.emit("local-model-error", event);
                }
            }
        });
        Ok(())
    }

    pub fn cancel(&self, id: &str) {
        if let Some(t) = self.downloads.lock().unwrap_or_else(|e| e.into_inner()).get(id) {
            t.cancel();
        }
    }

    /// Removes a catalog model's files.
    pub fn delete(&self, id: &str) -> Result<()> {
        self.catalog.get(id).map_err(|e| anyhow!("{e}"))?;
        self.store.remove(id).map_err(|e| anyhow!("{e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_kinds_map_to_what_the_ui_can_act_on() {
        let kind = |e: SttError| error_event("m", &e).kind;
        assert_eq!(kind(SttError::Cancelled), "cancelled");
        assert_eq!(kind(SttError::NotEnoughSpace { needed: 10, available: 5 }), "no_space");
        assert_eq!(kind(SttError::Network { what: "f".into(), message: "x".into() }), "network");
        assert_eq!(kind(SttError::HashMismatch { file: "f".into() }), "corrupt");
        assert_eq!(kind(SttError::Archive("x".into())), "corrupt");
        assert_eq!(kind(SttError::Engine("x".into())), "other");

        let e = error_event("m", &SttError::NotEnoughSpace { needed: 10, available: 5 });
        assert_eq!((e.needed_bytes, e.available_bytes), (Some(10), Some(5)));
    }

    #[test]
    fn phases_have_stable_names_for_the_frontend() {
        assert_eq!(phase_name(&Phase::Downloading), "downloading");
        assert_eq!(phase_name(&Phase::Verifying), "verifying");
        assert_eq!(phase_name(&Phase::Unpacking), "unpacking");
        assert_eq!(phase_name(&Phase::Done), "done");
    }

    #[test]
    fn lists_the_catalog_and_can_limit_it() {
        let dir = tempfile::tempdir().unwrap();
        let m = ModelManager::new(dir.path().to_path_buf()).unwrap();
        let all = m.list(None).unwrap();
        assert_eq!(all.len(), 7);
        assert!(all.iter().all(|x| x.status == "not_installed" && !x.custom));
        let some = m.list(Some(&["moonshine-tiny-en", "no-such-model"])).unwrap();
        assert_eq!(some.len(), 1);
        assert_eq!(some[0].id, "moonshine-tiny-en");
    }

    #[test]
    fn deleting_an_unknown_model_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let m = ModelManager::new(dir.path().to_path_buf()).unwrap();
        assert!(m.delete("no-such-model").is_err());
        m.delete("moonshine-tiny-en").unwrap();
    }
}
