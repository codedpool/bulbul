// SPDX-License-Identifier: GPL-3.0-only
// Copyright (c) 2026 Romanch Roshan Singh

//! Where downloaded models live, and whether a given model is complete.
//!
//! Each model gets its own folder under the store's root, named by its catalog
//! id. The downloader writes the files and, once every hash has checked out,
//! calls [`Store::mark_complete`]. A folder someone filled by hand (a manual
//! install, or one copied from another machine) still counts as installed when
//! all the expected files are present, but is reported as *not verified*.

use std::fs;
use std::path::{Path, PathBuf};

use crate::catalog::{is_safe_id, ModelEntry, Source};
use crate::error::{Error, Result};

/// Written into a model's folder after its files were downloaded and verified.
const MARKER: &str = ".complete";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    NotInstalled,
    /// Some files are there but not all (an interrupted download, or a file
    /// deleted afterwards).
    Incomplete,
    /// Everything the model needs is present. `verified` is true when Bulbul
    /// downloaded and hash-checked it, false for a manual install.
    Installed { verified: bool },
}

#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Store { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The model's folder. Refuses ids that could point outside the store.
    pub fn model_dir(&self, id: &str) -> Result<PathBuf> {
        if !is_safe_id(id) {
            return Err(Error::UnsafeId(id.to_string()));
        }
        Ok(self.root.join(id))
    }

    /// Absolute path of one of the model's files (or folders) by role.
    pub fn path_for(&self, entry: &ModelEntry, role: &str) -> Result<PathBuf> {
        let rel = entry
            .layout
            .get(role)
            .ok_or_else(|| Error::InvalidCatalog(format!("{} has no \"{role}\" file", entry.id)))?;
        Ok(self.model_dir(&entry.id)?.join(rel))
    }

    pub fn status(&self, entry: &ModelEntry) -> Result<Status> {
        let dir = self.model_dir(&entry.id)?;
        if !dir.is_dir() {
            return Ok(Status::NotInstalled);
        }
        let marker = dir.join(MARKER).is_file();

        let layout_ok = entry.layout.values().all(|rel| dir.join(rel).exists());
        // For per-file sources the expected sizes are known, so a truncated
        // file is caught without re-hashing gigabytes.
        let sizes_ok = match &entry.source {
            Source::Files { files, .. } => files.iter().all(|f| {
                fs::metadata(dir.join(&f.path)).map(|m| m.len() == f.size).unwrap_or(false)
            }),
            Source::Archive { .. } => true,
        };
        if layout_ok && sizes_ok {
            return Ok(Status::Installed { verified: marker });
        }

        let has_content = fs::read_dir(&dir)
            .map_err(|e| Error::io(&dir, e))?
            .filter_map(|e| e.ok())
            .any(|e| e.file_name() != MARKER);
        Ok(if has_content { Status::Incomplete } else { Status::NotInstalled })
    }

    /// Called by the downloader once everything is on disk and hash-checked.
    pub fn mark_complete(&self, entry: &ModelEntry) -> Result<()> {
        match self.status(entry)? {
            Status::Installed { .. } => {}
            _ => return Err(Error::NotInstalled(entry.id.clone())),
        }
        let path = self.model_dir(&entry.id)?.join(MARKER);
        fs::write(&path, entry.id.as_bytes()).map_err(|e| Error::io(path, e))
    }

    /// Deletes the model's folder. Fine to call when it isn't there.
    pub fn remove(&self, id: &str) -> Result<()> {
        let dir = self.model_dir(id)?;
        match fs::remove_dir_all(&dir) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(Error::io(dir, e)),
        }
    }

    /// Bytes currently used by the model's folder.
    pub fn disk_usage(&self, id: &str) -> Result<u64> {
        fn walk(p: &Path) -> std::io::Result<u64> {
            let mut total = 0;
            for e in fs::read_dir(p)? {
                let e = e?;
                let m = e.metadata()?;
                total += if m.is_dir() { walk(&e.path())? } else { m.len() };
            }
            Ok(total)
        }
        let dir = self.model_dir(id)?;
        if !dir.is_dir() {
            return Ok(0);
        }
        walk(&dir).map_err(|e| Error::io(dir, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{License, ModelKind, RemoteFile};
    use std::collections::BTreeMap;

    const H: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn entry() -> ModelEntry {
        ModelEntry {
            id: "tiny".into(),
            name: "Tiny".into(),
            summary: "x".into(),
            kind: ModelKind::Whisper,
            languages: vec![],
            multilingual: true,
            experimental: false,
            license: License {
                spdx: "MIT".into(),
                author: "a".into(),
                url: "https://example.com".into(),
                attribution: None,
                converted: false,
            },
            source: Source::Files {
                base_url: "https://example.com".into(),
                files: vec![
                    RemoteFile { path: "enc.onnx".into(), size: 4, sha256: H.into() },
                    RemoteFile { path: "tokens.txt".into(), size: 2, sha256: H.into() },
                ],
            },
            layout: BTreeMap::from([
                ("encoder".to_string(), "enc.onnx".to_string()),
                ("tokens".to_string(), "tokens.txt".to_string()),
            ]),
            download_bytes: 6,
            disk_bytes: 6,
        }
    }

    fn put(dir: &Path, name: &str, bytes: usize) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join(name), vec![0u8; bytes]).unwrap();
    }

    #[test]
    fn a_missing_folder_is_not_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let s = Store::new(tmp.path());
        assert_eq!(s.status(&entry()).unwrap(), Status::NotInstalled);
    }

    #[test]
    fn half_a_download_is_incomplete() {
        let tmp = tempfile::tempdir().unwrap();
        let s = Store::new(tmp.path());
        put(&s.model_dir("tiny").unwrap(), "enc.onnx", 4);
        assert_eq!(s.status(&entry()).unwrap(), Status::Incomplete);
    }

    #[test]
    fn a_truncated_file_is_incomplete_even_if_present() {
        let tmp = tempfile::tempdir().unwrap();
        let s = Store::new(tmp.path());
        let dir = s.model_dir("tiny").unwrap();
        put(&dir, "enc.onnx", 3); // expected 4
        put(&dir, "tokens.txt", 2);
        assert_eq!(s.status(&entry()).unwrap(), Status::Incomplete);
    }

    #[test]
    fn a_complete_folder_is_installed_but_unverified_until_marked() {
        let tmp = tempfile::tempdir().unwrap();
        let s = Store::new(tmp.path());
        let dir = s.model_dir("tiny").unwrap();
        put(&dir, "enc.onnx", 4);
        put(&dir, "tokens.txt", 2);
        assert_eq!(s.status(&entry()).unwrap(), Status::Installed { verified: false });
        s.mark_complete(&entry()).unwrap();
        assert_eq!(s.status(&entry()).unwrap(), Status::Installed { verified: true });
    }

    #[test]
    fn marking_an_incomplete_model_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let s = Store::new(tmp.path());
        put(&s.model_dir("tiny").unwrap(), "enc.onnx", 4);
        assert!(matches!(s.mark_complete(&entry()), Err(Error::NotInstalled(_))));
    }

    #[test]
    fn a_file_deleted_after_install_downgrades_the_status() {
        let tmp = tempfile::tempdir().unwrap();
        let s = Store::new(tmp.path());
        let dir = s.model_dir("tiny").unwrap();
        put(&dir, "enc.onnx", 4);
        put(&dir, "tokens.txt", 2);
        s.mark_complete(&entry()).unwrap();
        fs::remove_file(dir.join("tokens.txt")).unwrap();
        assert_eq!(s.status(&entry()).unwrap(), Status::Incomplete);
    }

    #[test]
    fn remove_deletes_the_folder_and_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let s = Store::new(tmp.path());
        let dir = s.model_dir("tiny").unwrap();
        put(&dir, "enc.onnx", 4);
        s.remove("tiny").unwrap();
        assert!(!dir.exists());
        s.remove("tiny").unwrap();
    }

    #[test]
    fn ids_that_could_escape_the_store_are_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let s = Store::new(tmp.path());
        for bad in ["..", "../x", "a/b", "", "/abs", "C:\\x"] {
            assert!(matches!(s.model_dir(bad), Err(Error::UnsafeId(_))), "{bad:?}");
            assert!(s.remove(bad).is_err(), "remove({bad:?}) must not touch the disk");
        }
    }

    #[test]
    fn paths_and_disk_usage() {
        let tmp = tempfile::tempdir().unwrap();
        let s = Store::new(tmp.path());
        let dir = s.model_dir("tiny").unwrap();
        put(&dir, "enc.onnx", 4);
        put(&dir.join("sub"), "x.bin", 10);
        assert_eq!(s.path_for(&entry(), "encoder").unwrap(), dir.join("enc.onnx"));
        assert!(s.path_for(&entry(), "nope").is_err());
        assert_eq!(s.disk_usage("tiny").unwrap(), 14);
        assert_eq!(s.disk_usage("absent").unwrap(), 0);
    }

    #[test]
    fn archive_models_use_layout_presence_only() {
        let tmp = tempfile::tempdir().unwrap();
        let s = Store::new(tmp.path());
        let mut e = entry();
        e.source = Source::Archive {
            url: "https://example.com/a.tar.bz2".into(),
            size: 6,
            sha256: H.into(),
            format: crate::catalog::ArchiveFormat::TarBz2,
            strip_prefix: None,
        };
        let dir = s.model_dir("tiny").unwrap();
        put(&dir, "enc.onnx", 1234); // any size: archives carry no per-file sizes
        put(&dir, "tokens.txt", 5);
        assert_eq!(s.status(&e).unwrap(), Status::Installed { verified: false });
    }
}
