// SPDX-License-Identifier: GPL-3.0-only
// Copyright (c) 2026 Romanch Roshan Singh

//! A stand-in transcriber for tests, so code that depends on [`Transcriber`]
//! can be exercised without a model or an engine.

use crate::error::{Error, Result};
use crate::transcriber::{Audio, LoadSpec, TranscribeOptions, Transcriber, Transcript};

#[derive(Debug, Default)]
pub struct FakeTranscriber {
    /// What `transcribe` returns.
    pub reply: String,
    /// Make the next `load` or `transcribe` fail once, with this message.
    pub fail_next: Option<String>,
    loaded: Option<String>,
    /// Options passed to the most recent `transcribe`.
    pub last_options: Option<TranscribeOptions>,
    /// Number of audio samples in the most recent `transcribe`.
    pub last_sample_count: Option<usize>,
}

impl FakeTranscriber {
    pub fn replying(text: &str) -> Self {
        FakeTranscriber { reply: text.to_string(), ..Default::default() }
    }
}

impl Transcriber for FakeTranscriber {
    fn load(&mut self, spec: &LoadSpec) -> Result<()> {
        if let Some(msg) = self.fail_next.take() {
            return Err(Error::Engine(msg));
        }
        self.loaded = Some(spec.model_id.clone());
        Ok(())
    }

    fn loaded_model(&self) -> Option<&str> {
        self.loaded.as_deref()
    }

    fn transcribe(&mut self, audio: &Audio, opts: &TranscribeOptions) -> Result<Transcript> {
        if self.loaded.is_none() {
            return Err(Error::NotLoaded);
        }
        if let Some(msg) = self.fail_next.take() {
            return Err(Error::Engine(msg));
        }
        self.last_options = Some(opts.clone());
        self.last_sample_count = Some(audio.samples.len());
        Ok(Transcript { text: self.reply.clone(), language: opts.language.clone() })
    }

    fn unload(&mut self) {
        self.loaded = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{Catalog, ModelKind};
    use std::collections::BTreeMap;

    fn audio() -> Audio {
        Audio { sample_rate: 16_000, samples: vec![0.0; 160] }
    }

    fn spec(id: &str) -> LoadSpec {
        LoadSpec { model_id: id.into(), kind: ModelKind::Whisper, paths: BTreeMap::new(), num_threads: None }
    }

    #[test]
    fn follows_the_load_transcribe_unload_lifecycle() {
        let cat = Catalog::bundled().unwrap();
        let entry = &cat.models[0];
        let mut t = FakeTranscriber::replying("hello there");
        let spec = spec(&entry.id);

        assert!(matches!(t.transcribe(&audio(), &Default::default()), Err(Error::NotLoaded)));

        t.load(&spec).unwrap();
        assert_eq!(t.loaded_model(), Some(entry.id.as_str()));

        let opts = TranscribeOptions { language: Some("hi".into()), hotwords: vec!["Bulbul".into()] };
        let out = t.transcribe(&audio(), &opts).unwrap();
        assert_eq!(out.text, "hello there");
        assert_eq!(out.language.as_deref(), Some("hi"));
        assert_eq!(t.last_options.as_ref().unwrap().hotwords, vec!["Bulbul"]);
        assert_eq!(t.last_sample_count, Some(160));

        t.unload();
        assert_eq!(t.loaded_model(), None);
        t.unload(); // harmless twice
    }

    #[test]
    fn can_be_told_to_fail_once() {
        let mut t = FakeTranscriber::replying("ok");
        t.fail_next = Some("device lost".into());
        assert!(t.load(&spec("m")).is_err());
        t.load(&spec("m")).unwrap();
        t.fail_next = Some("boom".into());
        assert!(t.transcribe(&audio(), &Default::default()).is_err());
        assert!(t.transcribe(&audio(), &Default::default()).is_ok());
    }
}
