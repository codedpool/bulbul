// SPDX-License-Identifier: GPL-3.0-only
// Copyright (c) 2026 Romanch Roshan Singh

//! The sherpa-onnx engine behind the [`Transcriber`] trait.

use bulbul_local_stt::chunk::{max_chunk_seconds, split_at_quiet_points};
use bulbul_local_stt::{
    Audio, Error, LoadSpec, ModelKind, Result, TranscribeOptions, Transcriber, Transcript,
};
use sherpa_onnx::{
    OfflineModelConfig, OfflineMoonshineModelConfig, OfflineNemoEncDecCtcModelConfig,
    OfflineOmnilingualAsrCtcModelConfig, OfflineQwen3ASRModelConfig, OfflineRecognizer,
    OfflineRecognizerConfig, OfflineTransducerModelConfig, OfflineWhisperModelConfig,
};

pub struct SherpaTranscriber {
    recognizer: Option<OfflineRecognizer>,
    spec: Option<LoadSpec>,
    /// The language the current Whisper recognizer was built for.
    whisper_language: Option<String>,
}

impl SherpaTranscriber {
    pub fn new() -> Self {
        SherpaTranscriber { recognizer: None, spec: None, whisper_language: None }
    }

    fn build(spec: &LoadSpec, language: Option<&str>) -> Result<OfflineRecognizer> {
        let cfg = recognizer_config(spec, language)?;
        OfflineRecognizer::create(&cfg).ok_or_else(|| {
            Error::Engine(format!(
                "the engine couldn't load {}. The model files may be damaged or incomplete.",
                spec.model_id
            ))
        })
    }
}

impl Transcriber for SherpaTranscriber {
    fn load(&mut self, spec: &LoadSpec) -> Result<()> {
        // Free the old model first so two big models are never in memory at once.
        self.unload();
        let recognizer = Self::build(spec, None)?;
        self.recognizer = Some(recognizer);
        self.spec = Some(spec.clone());
        self.whisper_language = None;
        Ok(())
    }

    fn loaded_model(&self) -> Option<&str> {
        self.spec.as_ref().map(|s| s.model_id.as_str())
    }

    fn transcribe(&mut self, audio: &Audio, opts: &TranscribeOptions) -> Result<Transcript> {
        let spec = self.spec.clone().ok_or(Error::NotLoaded)?;

        // Whisper takes its language when it's built, so a change of language
        // means rebuilding (about half a second, and only when it changes).
        if spec.kind == ModelKind::Whisper && opts.language != self.whisper_language {
            let rebuilt = Self::build(&spec, opts.language.as_deref())?;
            self.recognizer = Some(rebuilt);
            self.whisper_language = opts.language.clone();
        }
        let recognizer = self.recognizer.as_ref().ok_or(Error::NotLoaded)?;

        let max = max_chunk_seconds(spec.kind);
        let mut parts: Vec<String> = Vec::new();
        for piece in split_at_quiet_points(&audio.samples, audio.sample_rate, max) {
            let stream = recognizer.create_stream();
            stream.accept_waveform(audio.sample_rate as i32, piece);
            recognizer.decode(&stream);
            if let Some(r) = stream.get_result() {
                let t = r.text.trim();
                if !t.is_empty() {
                    parts.push(t.to_string());
                }
            }
        }
        Ok(Transcript { text: parts.join(" "), language: opts.language.clone() })
    }

    fn unload(&mut self) {
        self.recognizer = None;
        self.spec = None;
        self.whisper_language = None;
    }
}

/// Threads to use when the app doesn't say: a few, never all of them, so a
/// dictation doesn't make the rest of the computer crawl.
fn default_threads() -> i32 {
    std::thread::available_parallelism().map(|n| n.get().clamp(1, 4) as i32).unwrap_or(2)
}

/// Turns a model description into the engine's own configuration.
pub fn recognizer_config(spec: &LoadSpec, language: Option<&str>) -> Result<OfflineRecognizerConfig> {
    let path = |role: &str| -> Result<String> { Ok(spec.path(role)?.to_string_lossy().into_owned()) };

    let mut model = OfflineModelConfig {
        num_threads: spec.num_threads.map(|n| n.clamp(1, 16) as i32).unwrap_or_else(default_threads),
        debug: false,
        ..Default::default()
    };

    match spec.kind {
        ModelKind::Whisper => {
            model.whisper = OfflineWhisperModelConfig {
                encoder: Some(path("encoder")?),
                decoder: Some(path("decoder")?),
                language: language.map(str::to_string),
                task: Some("transcribe".into()),
                ..Default::default()
            };
            model.tokens = Some(path("tokens")?);
        }
        ModelKind::Transducer => {
            model.transducer = OfflineTransducerModelConfig {
                encoder: Some(path("encoder")?),
                decoder: Some(path("decoder")?),
                joiner: Some(path("joiner")?),
            };
            model.tokens = Some(path("tokens")?);
            // The transducers in the catalog are NVIDIA NeMo models.
            model.model_type = Some("nemo_transducer".into());
        }
        ModelKind::NemoCtc => {
            model.nemo_ctc = OfflineNemoEncDecCtcModelConfig { model: Some(path("model")?) };
            model.tokens = Some(path("tokens")?);
        }
        ModelKind::Moonshine => {
            model.moonshine = OfflineMoonshineModelConfig {
                encoder: Some(path("encoder")?),
                merged_decoder: Some(path("merged_decoder")?),
                ..Default::default()
            };
            model.tokens = Some(path("tokens")?);
        }
        ModelKind::Qwen3Asr => {
            model.qwen3_asr = OfflineQwen3ASRModelConfig {
                conv_frontend: Some(path("conv_frontend")?),
                encoder: Some(path("encoder")?),
                decoder: Some(path("decoder")?),
                tokenizer: Some(path("tokenizer")?),
                ..Default::default()
            };
        }
        ModelKind::OmnilingualCtc => {
            model.omnilingual = OfflineOmnilingualAsrCtcModelConfig { model: Some(path("model")?) };
            model.tokens = Some(path("tokens")?);
        }
    }

    Ok(OfflineRecognizerConfig { model_config: model, ..Default::default() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn spec(kind: ModelKind, roles: &[&str]) -> LoadSpec {
        LoadSpec {
            model_id: "m".into(),
            kind,
            paths: roles.iter().map(|r| (r.to_string(), PathBuf::from(format!("/store/m/{r}.bin")))).collect::<BTreeMap<_, _>>(),
            num_threads: Some(3),
        }
    }

    #[test]
    fn whisper_gets_its_files_task_and_language() {
        let cfg = recognizer_config(&spec(ModelKind::Whisper, &["encoder", "decoder", "tokens"]), Some("hi")).unwrap();
        let m = &cfg.model_config;
        assert_eq!(m.whisper.language.as_deref(), Some("hi"));
        assert_eq!(m.whisper.task.as_deref(), Some("transcribe"));
        assert!(m.whisper.encoder.as_deref().unwrap().ends_with("encoder.bin"));
        assert!(m.tokens.is_some());
        assert_eq!(m.num_threads, 3);
        assert!(!m.debug);
        // No language means "detect it".
        let auto = recognizer_config(&spec(ModelKind::Whisper, &["encoder", "decoder", "tokens"]), None).unwrap();
        assert_eq!(auto.model_config.whisper.language, None);
    }

    #[test]
    fn each_family_maps_to_its_own_config() {
        let t = recognizer_config(&spec(ModelKind::Transducer, &["encoder", "decoder", "joiner", "tokens"]), None).unwrap();
        assert!(t.model_config.transducer.joiner.is_some());
        assert_eq!(t.model_config.model_type.as_deref(), Some("nemo_transducer"));

        let c = recognizer_config(&spec(ModelKind::NemoCtc, &["model", "tokens"]), None).unwrap();
        assert!(c.model_config.nemo_ctc.model.is_some());

        let mo = recognizer_config(&spec(ModelKind::Moonshine, &["encoder", "merged_decoder", "tokens"]), None).unwrap();
        assert!(mo.model_config.moonshine.merged_decoder.is_some());

        let q = recognizer_config(&spec(ModelKind::Qwen3Asr, &["conv_frontend", "encoder", "decoder", "tokenizer"]), None).unwrap();
        assert!(q.model_config.qwen3_asr.tokenizer.is_some());
        assert!(q.model_config.tokens.is_none(), "Qwen3-ASR uses its tokenizer folder, not a tokens file");

        let o = recognizer_config(&spec(ModelKind::OmnilingualCtc, &["model", "tokens"]), None).unwrap();
        assert!(o.model_config.omnilingual.model.is_some());
    }

    #[test]
    fn a_missing_file_role_is_named_in_the_error() {
        let err = match recognizer_config(&spec(ModelKind::Transducer, &["encoder", "decoder", "tokens"]), None) {
            Err(e) => e.to_string(),
            Ok(_) => panic!("should have failed"),
        };
        assert!(err.contains("joiner"), "{err}");
    }

    #[test]
    fn thread_counts_are_kept_sensible() {
        let mut s = spec(ModelKind::NemoCtc, &["model", "tokens"]);
        s.num_threads = Some(999);
        assert_eq!(recognizer_config(&s, None).unwrap().model_config.num_threads, 16);
        s.num_threads = Some(0);
        assert_eq!(recognizer_config(&s, None).unwrap().model_config.num_threads, 1);
        s.num_threads = None;
        let n = recognizer_config(&s, None).unwrap().model_config.num_threads;
        assert!((1..=4).contains(&n));
    }
}
