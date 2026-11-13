// SPDX-License-Identifier: GPL-3.0-only
// Copyright (c) 2026 Romanch Roshan Singh

//! Models the user brings themselves: a folder of files plus a choice of model
//! family. Nothing here is checked against a hash (there is nothing to check it
//! against), so the app always labels these as unverified.
//!
//! The files are found by name, the way sherpa-onnx's own downloads are laid
//! out (`encoder*.onnx`, `tokens.txt`, ...). When both a full-precision and an
//! `int8` file exist, the int8 one wins: it loads faster and uses less memory.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::catalog::ModelKind;
use crate::error::{Error, Result};
use crate::transcriber::LoadSpec;

/// What one role looks for in a folder.
struct Want {
    role: &'static str,
    /// The file name must contain one of these (lowercase).
    contains: &'static [&'static str],
    /// ...and end with one of these.
    extensions: &'static [&'static str],
    /// A directory rather than a file (Qwen3's tokenizer).
    dir: bool,
}

const fn file(role: &'static str, contains: &'static [&'static str], extensions: &'static [&'static str]) -> Want {
    Want { role, contains, extensions, dir: false }
}

fn wants(kind: ModelKind) -> Vec<Want> {
    const ONNX: &[&str] = &[".onnx"];
    const ONNX_ORT: &[&str] = &[".onnx", ".ort"];
    match kind {
        ModelKind::Whisper => vec![
            file("encoder", &["encoder"], ONNX),
            file("decoder", &["decoder"], ONNX),
            file("tokens", &["tokens"], &[".txt"]),
        ],
        ModelKind::Transducer => vec![
            file("encoder", &["encoder"], ONNX),
            file("decoder", &["decoder"], ONNX),
            file("joiner", &["joiner"], ONNX),
            file("tokens", &["tokens"], &[".txt"]),
        ],
        ModelKind::NemoCtc | ModelKind::OmnilingualCtc => {
            vec![file("model", &["model"], ONNX), file("tokens", &["tokens"], &[".txt"])]
        }
        ModelKind::Moonshine => vec![
            file("encoder", &["encoder"], ONNX_ORT),
            file("merged_decoder", &["decoder"], ONNX_ORT),
            file("tokens", &["tokens"], &[".txt"]),
        ],
        ModelKind::Qwen3Asr => vec![
            file("conv_frontend", &["conv_frontend"], ONNX),
            file("encoder", &["encoder"], ONNX),
            file("decoder", &["decoder"], ONNX),
            Want { role: "tokenizer", contains: &["tokenizer"], extensions: &[], dir: true },
        ],
    }
}

/// Picks the best match for one role among the folder's entries.
fn pick(want: &Want, entries: &[(String, PathBuf, bool)]) -> Option<PathBuf> {
    let mut found: Vec<&(String, PathBuf, bool)> = entries
        .iter()
        .filter(|(name, _, is_dir)| {
            *is_dir == want.dir
                && want.contains.iter().any(|c| name.contains(c))
                && (want.dir || want.extensions.iter().any(|e| name.ends_with(e)))
                // A merged Moonshine decoder is the one we want, not the
                // separate uncached one.
                && !(want.role == "merged_decoder" && !name.contains("merged") && entries.iter().any(|(n, ..)| n.contains("decoder") && n.contains("merged")))
        })
        .collect();
    // int8 first, then the shortest name (the least decorated one).
    found.sort_by_key(|(name, ..)| (!name.contains("int8"), name.len()));
    found.first().map(|(_, p, _)| p.clone())
}

/// Finds the files a model of `kind` needs inside `dir`.
pub fn detect_layout(kind: ModelKind, dir: &Path) -> Result<BTreeMap<String, PathBuf>> {
    let read = std::fs::read_dir(dir).map_err(|e| Error::io(dir, e))?;
    let mut entries = Vec::new();
    for item in read {
        let item = item.map_err(|e| Error::io(dir, e))?;
        let name = item.file_name().to_string_lossy().to_lowercase();
        let is_dir = item.file_type().map(|t| t.is_dir()).unwrap_or(false);
        entries.push((name, item.path(), is_dir));
    }

    let mut layout = BTreeMap::new();
    let mut missing = Vec::new();
    for want in wants(kind) {
        match pick(&want, &entries) {
            Some(p) => {
                layout.insert(want.role.to_string(), p);
            }
            None => missing.push(want.role),
        }
    }
    if missing.is_empty() {
        Ok(layout)
    } else {
        Err(Error::Engine(format!(
            "that folder doesn't look like a {} model: it's missing {}",
            kind_label(kind),
            missing.join(", ")
        )))
    }
}

pub fn kind_label(kind: ModelKind) -> &'static str {
    match kind {
        ModelKind::Whisper => "Whisper",
        ModelKind::Transducer => "transducer (Parakeet-style)",
        ModelKind::NemoCtc => "NeMo CTC",
        ModelKind::Moonshine => "Moonshine",
        ModelKind::Qwen3Asr => "Qwen3-ASR",
        ModelKind::OmnilingualCtc => "Omnilingual CTC",
    }
}

impl LoadSpec {
    /// Builds the spec for a model kept in a folder the user chose.
    pub fn from_folder(id: &str, kind: ModelKind, dir: &Path, num_threads: Option<u32>) -> Result<LoadSpec> {
        Ok(LoadSpec { model_id: id.to_string(), kind, paths: detect_layout(kind, dir)?, num_threads })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(files: &[&str], dirs: &[&str]) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        for f in files {
            std::fs::write(d.path().join(f), b"x").unwrap();
        }
        for n in dirs {
            std::fs::create_dir(d.path().join(n)).unwrap();
        }
        d
    }

    fn names(l: &BTreeMap<String, PathBuf>) -> BTreeMap<String, String> {
        l.iter().map(|(k, v)| (k.clone(), v.file_name().unwrap().to_string_lossy().into_owned())).collect()
    }

    #[test]
    fn finds_a_whisper_model_and_prefers_int8() {
        let d = folder(
            &["small-encoder.onnx", "small-encoder.int8.onnx", "small-decoder.int8.onnx", "small-tokens.txt", "README.md"],
            &[],
        );
        let l = names(&detect_layout(ModelKind::Whisper, d.path()).unwrap());
        assert_eq!(l["encoder"], "small-encoder.int8.onnx");
        assert_eq!(l["decoder"], "small-decoder.int8.onnx");
        assert_eq!(l["tokens"], "small-tokens.txt");
    }

    #[test]
    fn finds_a_transducer() {
        let d = folder(&["encoder.int8.onnx", "decoder.int8.onnx", "joiner.int8.onnx", "tokens.txt"], &[]);
        assert_eq!(detect_layout(ModelKind::Transducer, d.path()).unwrap().len(), 4);
    }

    #[test]
    fn finds_ctc_and_moonshine_and_qwen() {
        let d = folder(&["model.int8.onnx", "tokens.txt"], &[]);
        assert_eq!(detect_layout(ModelKind::OmnilingualCtc, d.path()).unwrap().len(), 2);
        assert_eq!(detect_layout(ModelKind::NemoCtc, d.path()).unwrap().len(), 2);

        let m = folder(&["encoder_model.ort", "decoder_model.ort", "decoder_model_merged.ort", "tokens.txt"], &[]);
        let l = names(&detect_layout(ModelKind::Moonshine, m.path()).unwrap());
        assert_eq!(l["merged_decoder"], "decoder_model_merged.ort");

        let q = folder(&["conv_frontend.onnx", "encoder.int8.onnx", "decoder.int8.onnx"], &["tokenizer"]);
        assert_eq!(detect_layout(ModelKind::Qwen3Asr, q.path()).unwrap().len(), 4);
    }

    #[test]
    fn a_folder_of_the_wrong_kind_names_what_is_missing() {
        let d = folder(&["model.int8.onnx", "tokens.txt"], &[]);
        let err = detect_layout(ModelKind::Transducer, d.path()).unwrap_err().to_string();
        assert!(err.contains("encoder") && err.contains("joiner"), "{err}");
        assert!(err.contains("transducer"), "{err}");
    }

    #[test]
    fn a_missing_folder_is_an_error() {
        assert!(detect_layout(ModelKind::Whisper, Path::new("/definitely/not/here")).is_err());
    }

    #[test]
    fn builds_a_load_spec() {
        let d = folder(&["model.onnx", "tokens.txt"], &[]);
        let spec = LoadSpec::from_folder("custom:x", ModelKind::NemoCtc, d.path(), Some(2)).unwrap();
        assert_eq!(spec.model_id, "custom:x");
        assert_eq!(spec.num_threads, Some(2));
        assert!(spec.path("model").is_ok());
    }
}
