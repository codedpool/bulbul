// SPDX-License-Identifier: GPL-3.0-only
// Copyright (c) 2026 Romanch Roshan Singh

//! The model catalog: every on-device model Bulbul offers, where to download
//! it from, and how to check what arrived.
//!
//! `catalog.json` is *generated* (see `scripts/stt-catalog/`), never edited by
//! hand: sizes and hashes come from the real files. [`Catalog::validate`]
//! enforces the rules the app relies on, including the licence policy.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Catalog format version this code understands.
pub const SCHEMA: u32 = 1;

/// The catalog that ships inside the app.
const BUNDLED: &str = include_str!("../catalog.json");

/// Licences the app will offer. Anything else (non-commercial, share-alike,
/// "other") is refused at validation time rather than reviewed model by model.
const ALLOWED_LICENCES: &[&str] = &["MIT", "Apache-2.0", "CC-BY-4.0"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Catalog {
    pub schema: u32,
    /// Date the generator ran, `YYYY-MM-DD`. Informational.
    pub generated: String,
    pub models: Vec<ModelEntry>,
}

/// Which family of model this is; decides how the engine loads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelKind {
    Whisper,
    /// Encoder / decoder / joiner (Parakeet and friends).
    Transducer,
    NemoCtc,
    Moonshine,
    Qwen3Asr,
    OmnilingualCtc,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelEntry {
    pub id: String,
    pub name: String,
    /// One sentence for the model's card.
    pub summary: String,
    pub kind: ModelKind,
    /// ISO 639-1 codes the model is meant for. Empty with `multilingual`
    /// set means "many languages; see the model's own docs".
    pub languages: Vec<String>,
    pub multilingual: bool,
    pub experimental: bool,
    pub license: License,
    pub source: Source,
    /// Role (`encoder`, `decoder`, `joiner`, `tokens`, `model`, ...) to the
    /// file's path inside the model folder once installed.
    pub layout: BTreeMap<String, String>,
    /// Bytes to download.
    pub download_bytes: u64,
    /// Bytes on disk once installed.
    pub disk_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct License {
    /// SPDX identifier, e.g. `Apache-2.0`.
    pub spdx: String,
    /// Who made the original model.
    pub author: String,
    pub url: String,
    /// The credit line to show next to the model, when the licence requires
    /// one (CC-BY). `None` otherwise.
    pub attribution: Option<String>,
    /// True when the downloaded files are a conversion (for example to ONNX or
    /// int8) of the original, which CC-BY says must be indicated.
    pub converted: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Source {
    /// Individual files, each hashed on its own (resumable one by one).
    Files { base_url: String, files: Vec<RemoteFile> },
    /// One compressed archive, hashed as a whole and unpacked after download.
    /// Only the listed `files` are extracted, and each is checked on its own.
    Archive {
        url: String,
        size: u64,
        sha256: String,
        format: ArchiveFormat,
        /// Leading folder inside the archive to drop when unpacking.
        strip_prefix: Option<String>,
        /// The files to extract, as paths after `strip_prefix` is removed.
        files: Vec<RemoteFile>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoteFile {
    /// Path relative to `base_url` (or the archive) and to the model folder.
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveFormat {
    TarBz2,
}

impl Catalog {
    /// The catalog bundled with this build. Validated; a failure here is a
    /// packaging bug, caught by the tests.
    pub fn bundled() -> Result<Catalog> {
        Self::from_json(BUNDLED)
    }

    pub fn from_json(json: &str) -> Result<Catalog> {
        let catalog: Catalog = serde_json::from_str(json)
            .map_err(|e| Error::InvalidCatalog(format!("not valid JSON for a catalog: {e}")))?;
        catalog.validate()?;
        Ok(catalog)
    }

    pub fn get(&self, id: &str) -> Result<&ModelEntry> {
        self.models
            .iter()
            .find(|m| m.id == id)
            .ok_or_else(|| Error::UnknownModel(id.to_string()))
    }

    pub fn validate(&self) -> Result<()> {
        let bad = |msg: String| Err(Error::InvalidCatalog(msg));
        if self.schema != SCHEMA {
            return bad(format!("schema {} isn't supported (expected {SCHEMA})", self.schema));
        }
        if self.models.is_empty() {
            return bad("the catalog has no models".into());
        }
        let mut seen = HashSet::new();
        for m in &self.models {
            if !is_safe_id(&m.id) {
                return bad(format!("\"{}\" isn't a valid model id", m.id));
            }
            if !seen.insert(m.id.as_str()) {
                return bad(format!("duplicate model id \"{}\"", m.id));
            }
            m.validate().map_err(|e| match e {
                Error::InvalidCatalog(msg) => Error::InvalidCatalog(format!("{}: {msg}", m.id)),
                other => other,
            })?;
        }
        Ok(())
    }
}

impl ModelEntry {
    fn validate(&self) -> Result<()> {
        let bad = |msg: &str| Err(Error::InvalidCatalog(msg.to_string()));
        if self.name.trim().is_empty() || self.summary.trim().is_empty() {
            return bad("name and summary are required");
        }
        if !ALLOWED_LICENCES.contains(&self.license.spdx.as_str()) {
            return Err(Error::InvalidCatalog(format!(
                "licence {} isn't allowed (only {})",
                self.license.spdx,
                ALLOWED_LICENCES.join(", ")
            )));
        }
        if self.license.spdx == "CC-BY-4.0"
            && self.license.attribution.as_deref().map_or(true, |a| a.trim().is_empty())
        {
            return bad("a CC-BY-4.0 model needs its attribution text");
        }
        if !self.license.url.starts_with("https://") {
            return bad("the licence URL must be https");
        }
        if self.layout.is_empty() {
            return bad("the layout is empty");
        }
        if self.disk_bytes == 0 || self.download_bytes == 0 {
            return bad("download_bytes and disk_bytes must be set");
        }

        match &self.source {
            Source::Files { base_url, .. } => {
                if !base_url.starts_with("https://") {
                    return bad("base_url must be https");
                }
            }
            Source::Archive { url, size, sha256, strip_prefix, .. } => {
                if !url.starts_with("https://") {
                    return bad("archive url must be https");
                }
                if *size == 0 || !is_sha256(sha256) {
                    return bad("archive needs a size and a sha256");
                }
                if let Some(p) = strip_prefix {
                    if !is_safe_rel_path(p) {
                        return bad("unsafe strip_prefix");
                    }
                }
                if *size != self.download_bytes {
                    return bad("download_bytes must equal the archive size");
                }
            }
        }

        let files = self.files();
        if files.is_empty() {
            return bad("no files listed");
        }
        let mut total = 0u64;
        let mut names = HashSet::new();
        for f in files {
            if !is_safe_rel_path(&f.path) {
                return Err(Error::InvalidCatalog(format!("unsafe file path \"{}\"", f.path)));
            }
            if !names.insert(f.path.as_str()) {
                return Err(Error::InvalidCatalog(format!("duplicate file \"{}\"", f.path)));
            }
            if f.size == 0 {
                return Err(Error::InvalidCatalog(format!("file \"{}\" has no size", f.path)));
            }
            if !is_sha256(&f.sha256) {
                return Err(Error::InvalidCatalog(format!("file \"{}\" has a bad sha256", f.path)));
            }
            total += f.size;
        }
        if total != self.disk_bytes {
            return Err(Error::InvalidCatalog(format!(
                "file sizes add up to {total}, but disk_bytes is {}",
                self.disk_bytes
            )));
        }
        if matches!(self.source, Source::Files { .. }) && total != self.download_bytes {
            return Err(Error::InvalidCatalog(format!(
                "file sizes add up to {total}, but download_bytes is {}",
                self.download_bytes
            )));
        }
        // Each role must name a listed file, or a folder that holds listed files.
        for (role, path) in &self.layout {
            if !is_safe_rel_path(path) {
                return Err(Error::InvalidCatalog(format!("unsafe layout path \"{path}\"")));
            }
            let is_file = names.contains(path.as_str());
            let is_dir = files.iter().any(|f| f.path.starts_with(&format!("{path}/")));
            if !is_file && !is_dir {
                return Err(Error::InvalidCatalog(format!(
                    "layout role \"{role}\" points at \"{path}\", which isn't a listed file"
                )));
            }
        }
        Ok(())
    }

    /// Every file the model consists of once installed, however it's delivered.
    pub fn files(&self) -> &[RemoteFile] {
        match &self.source {
            Source::Files { files, .. } | Source::Archive { files, .. } => files,
        }
    }

    /// Whether the licence asks for credit to be shown with this model.
    pub fn needs_attribution(&self) -> bool {
        self.license.attribution.is_some()
    }
}

/// Lowercase letters, digits, `-`, `_` and `.` only, not starting with a dot:
/// safe to use as a folder name.
pub fn is_safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && !id.starts_with('.')
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.'))
}

/// A relative path that can't climb out of its folder.
pub fn is_safe_rel_path(p: &str) -> bool {
    !p.is_empty()
        && !p.starts_with('/')
        && !p.contains('\\')
        && !p.contains(':')
        && p.split('/').all(|seg| !seg.is_empty() && seg != "." && seg != "..")
}

fn is_sha256(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn files_entry(id: &str) -> ModelEntry {
        ModelEntry {
            id: id.into(),
            name: "Test model".into(),
            summary: "A model for tests.".into(),
            kind: ModelKind::Whisper,
            languages: vec!["en".into()],
            multilingual: false,
            experimental: false,
            license: License {
                spdx: "Apache-2.0".into(),
                author: "Someone".into(),
                url: "https://example.com/licence".into(),
                attribution: None,
                converted: false,
            },
            source: Source::Files {
                base_url: "https://example.com/m".into(),
                files: vec![
                    RemoteFile { path: "enc.onnx".into(), size: 100, sha256: H.into() },
                    RemoteFile { path: "tokens.txt".into(), size: 20, sha256: H.into() },
                ],
            },
            layout: BTreeMap::from([
                ("encoder".to_string(), "enc.onnx".to_string()),
                ("tokens".to_string(), "tokens.txt".to_string()),
            ]),
            download_bytes: 120,
            disk_bytes: 120,
        }
    }

    fn catalog_of(models: Vec<ModelEntry>) -> Catalog {
        Catalog { schema: SCHEMA, generated: "2026-11-13".into(), models }
    }

    #[test]
    fn a_well_formed_catalog_validates() {
        catalog_of(vec![files_entry("a-model")]).validate().unwrap();
    }

    #[test]
    fn the_bundled_catalog_is_valid_and_follows_the_policy() {
        let c = Catalog::bundled().unwrap();
        assert!(c.models.len() >= 7, "the seven curated models");
        for m in &c.models {
            assert!(ALLOWED_LICENCES.contains(&m.license.spdx.as_str()), "{}", m.id);
            if m.license.spdx == "CC-BY-4.0" {
                assert!(m.needs_attribution(), "{} must carry its credit", m.id);
                assert!(m.license.converted, "{} is a conversion and must say so", m.id);
            }
        }
    }

    #[test]
    fn ids_must_be_safe_and_unique() {
        for bad in ["", "Has Space", "../up", ".hidden", "UPPER", "a/b"] {
            assert!(!is_safe_id(bad), "{bad:?}");
            assert!(catalog_of(vec![files_entry(bad)]).validate().is_err(), "{bad:?}");
        }
        let dup = catalog_of(vec![files_entry("same"), files_entry("same")]);
        assert!(dup.validate().unwrap_err().to_string().contains("duplicate"));
    }

    #[test]
    fn unsafe_paths_are_rejected() {
        for bad in ["", "/abs", "a/../b", "..", "a\\b", "C:evil", "a//b", "./a"] {
            assert!(!is_safe_rel_path(bad), "{bad:?}");
        }
        assert!(is_safe_rel_path("sub/dir/file.onnx"));
        let mut e = files_entry("m");
        if let Source::Files { files, .. } = &mut e.source {
            files[0].path = "../escape.onnx".into();
        }
        assert!(catalog_of(vec![e]).validate().is_err());
    }

    #[test]
    fn disallowed_licences_are_refused() {
        for spdx in ["CC-BY-NC-4.0", "CC-BY-SA-4.0", "other", "GPL-3.0-only"] {
            let mut e = files_entry("m");
            e.license.spdx = spdx.into();
            let err = catalog_of(vec![e]).validate().unwrap_err().to_string();
            assert!(err.contains("isn't allowed"), "{spdx}: {err}");
        }
    }

    #[test]
    fn cc_by_needs_attribution_text() {
        let mut e = files_entry("m");
        e.license.spdx = "CC-BY-4.0".into();
        assert!(catalog_of(vec![e.clone()]).validate().is_err());
        e.license.attribution = Some("(c) Someone, CC-BY-4.0".into());
        catalog_of(vec![e]).validate().unwrap();
    }

    #[test]
    fn sizes_and_hashes_must_add_up() {
        let mut e = files_entry("m");
        e.download_bytes = 999;
        assert!(catalog_of(vec![e]).validate().is_err());

        let mut e = files_entry("m");
        if let Source::Files { files, .. } = &mut e.source {
            files[0].sha256 = "nothex".into();
        }
        assert!(catalog_of(vec![e]).validate().is_err());

        let mut e = files_entry("m");
        e.layout.insert("decoder".into(), "not-listed.onnx".into());
        assert!(catalog_of(vec![e]).validate().unwrap_err().to_string().contains("isn't a listed file"));
    }

    #[test]
    fn archive_sources_validate() {
        let mut e = files_entry("m");
        e.source = Source::Archive {
            url: "https://example.com/m.tar.bz2".into(),
            size: 5000,
            sha256: H.into(),
            format: ArchiveFormat::TarBz2,
            strip_prefix: Some("m-dir".into()),
            files: vec![
                RemoteFile { path: "enc.onnx".into(), size: 100, sha256: H.into() },
                RemoteFile { path: "tokens.txt".into(), size: 20, sha256: H.into() },
            ],
        };
        e.download_bytes = 5000; // compressed
        e.disk_bytes = 120; // extracted: the listed files
        catalog_of(vec![e.clone()]).validate().unwrap();
        // The download size has to match the archive...
        e.download_bytes = 4999;
        assert!(catalog_of(vec![e.clone()]).validate().is_err());
        // ...and the on-disk size the listed files.
        e.download_bytes = 5000;
        e.disk_bytes = 9000;
        assert!(catalog_of(vec![e]).validate().is_err());
    }

    #[test]
    fn a_layout_role_may_name_a_folder_of_listed_files() {
        let mut e = files_entry("m");
        if let Source::Files { files, .. } = &mut e.source {
            files.push(RemoteFile { path: "tok/vocab.json".into(), size: 5, sha256: H.into() });
        }
        e.download_bytes = 125;
        e.disk_bytes = 125;
        e.layout.insert("tokenizer".into(), "tok".into());
        catalog_of(vec![e.clone()]).validate().unwrap();
        e.layout.insert("tokenizer".into(), "nothere".into());
        assert!(catalog_of(vec![e]).validate().is_err());
    }

    #[test]
    fn schema_mismatch_and_garbage_are_rejected() {
        let mut c = catalog_of(vec![files_entry("m")]);
        c.schema = 99;
        assert!(c.validate().is_err());
        assert!(Catalog::from_json("{ nope").is_err());
        assert!(catalog_of(vec![]).validate().is_err());
    }

    #[test]
    fn lookup_by_id() {
        let c = catalog_of(vec![files_entry("a"), files_entry("b")]);
        assert_eq!(c.get("b").unwrap().id, "b");
        assert!(matches!(c.get("zzz"), Err(Error::UnknownModel(_))));
    }
}
