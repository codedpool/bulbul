// SPDX-License-Identifier: GPL-3.0-only
// Copyright (c) 2026 Romanch Roshan Singh

//! A stand-in transcriber for tests, so code that depends on [`Transcriber`]
//! can be exercised without a model or an engine.

use std::path::Path;

use crate::catalog::ModelEntry;
use crate::error::{Error, Result};
use crate::transcriber::{Audio, LoadOptions, TranscribeOptions, Transcriber, Transcript};

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
    fn load(&mut self, entry: &ModelEntry, _dir: &Path, _opts: &LoadOptions) -> Result<()> {
        if let Some(msg) = self.fail_next.take() {
            return Err(Error::Engine(msg));
        }
        self.loaded = Some(entry.id.clone());
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
    use crate::catalog::Catalog;

    fn audio() -> Audio {
        Audio { sample_rate: 16_000, samples: vec![0.0; 160] }
    }

    #[test]
    fn follows_the_load_transcribe_unload_lifecycle() {
        let cat = Catalog::bundled().unwrap();
        let entry = &cat.models[0];
        let mut t = FakeTranscriber::replying("hello there");

        assert!(matches!(t.transcribe(&audio(), &Default::default()), Err(Error::NotLoaded)));

        t.load(entry, Path::new("/unused"), &LoadOptions::default()).unwrap();
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
        let cat = Catalog::bundled().unwrap();
        let mut t = FakeTranscriber::replying("ok");
        t.fail_next = Some("device lost".into());
        assert!(t.load(&cat.models[0], Path::new("/x"), &LoadOptions::default()).is_err());
        t.load(&cat.models[0], Path::new("/x"), &LoadOptions::default()).unwrap();
        t.fail_next = Some("boom".into());
        assert!(t.transcribe(&audio(), &Default::default()).is_err());
        assert!(t.transcribe(&audio(), &Default::default()).is_ok());
    }
}
