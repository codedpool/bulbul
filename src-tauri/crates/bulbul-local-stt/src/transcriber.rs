// SPDX-License-Identifier: GPL-3.0-only
// Copyright (c) 2026 Romanch Roshan Singh

//! The interface between Bulbul and whatever runs a speech model.
//!
//! The real engine lives in a separate worker program; this trait is what the
//! app (and the worker, on its side of the pipe) code against, so the engine
//! can be swapped, or faked in tests, without touching callers.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::catalog::{ModelEntry, ModelKind};
use crate::error::{Error, Result};
use crate::store::Store;

/// Mono audio as floats in -1.0..=1.0.
#[derive(Debug, Clone, PartialEq)]
pub struct Audio {
    pub sample_rate: u32,
    pub samples: Vec<f32>,
}

impl Audio {
    pub fn duration_ms(&self) -> u64 {
        if self.sample_rate == 0 {
            return 0;
        }
        self.samples.len() as u64 * 1000 / self.sample_rate as u64
    }

    /// Reads a 16-bit PCM WAV (what Bulbul's recorder writes), mixing stereo
    /// down to mono. Anything else is refused with a clear reason.
    pub fn from_wav_bytes(bytes: &[u8]) -> Result<Audio> {
        let bad = |m: &str| Err(Error::BadAudio(m.to_string()));
        if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
            return bad("not a WAV file");
        }
        let (mut channels, mut rate, mut bits, mut format) = (0u16, 0u32, 0u16, 0u16);
        let mut pcm: Option<&[u8]> = None;
        let mut pos = 12usize;
        while pos + 8 <= bytes.len() {
            let id = &bytes[pos..pos + 4];
            let len = u32::from_le_bytes([bytes[pos + 4], bytes[pos + 5], bytes[pos + 6], bytes[pos + 7]]) as usize;
            let start = pos + 8;
            let end = start.saturating_add(len).min(bytes.len());
            if id == b"fmt " && end - start >= 16 {
                let f = &bytes[start..end];
                format = u16::from_le_bytes([f[0], f[1]]);
                channels = u16::from_le_bytes([f[2], f[3]]);
                rate = u32::from_le_bytes([f[4], f[5], f[6], f[7]]);
                bits = u16::from_le_bytes([f[14], f[15]]);
            } else if id == b"data" {
                pcm = Some(&bytes[start..end]);
                break;
            }
            // Chunks are padded to an even length.
            pos = start.saturating_add(len).saturating_add(len & 1);
        }
        let Some(pcm) = pcm else { return bad("the WAV has no audio data") };
        if format != 1 || bits != 16 {
            return bad("only 16-bit PCM WAV is supported");
        }
        if !(1..=2).contains(&channels) || rate == 0 {
            return bad("the WAV has an unsupported channel count or sample rate");
        }
        let frame = 2 * channels as usize;
        let samples = pcm
            .chunks_exact(frame)
            .map(|f| {
                let s0 = i16::from_le_bytes([f[0], f[1]]) as f32 / 32768.0;
                if channels == 2 {
                    (s0 + i16::from_le_bytes([f[2], f[3]]) as f32 / 32768.0) / 2.0
                } else {
                    s0
                }
            })
            .collect();
        Ok(Audio { sample_rate: rate, samples })
    }
}

/// Everything an engine needs to load a model: its family, and where each of
/// its files is. Deliberately independent of the catalog, so the worker
/// program never needs one.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadSpec {
    /// Catalog id, for reporting.
    pub model_id: String,
    pub kind: ModelKind,
    /// Role (`encoder`, `tokens`, ...) to absolute path.
    pub paths: BTreeMap<String, PathBuf>,
    /// CPU threads for inference. `None` lets the engine decide.
    pub num_threads: Option<u32>,
}

impl LoadSpec {
    /// Builds the spec for an installed catalog model.
    pub fn from_entry(store: &Store, entry: &ModelEntry, num_threads: Option<u32>) -> Result<LoadSpec> {
        let mut paths = BTreeMap::new();
        for role in entry.layout.keys() {
            paths.insert(role.clone(), store.path_for(entry, role)?);
        }
        Ok(LoadSpec { model_id: entry.id.clone(), kind: entry.kind, paths, num_threads })
    }

    /// The path for `role`, or a clear error if the spec doesn't have it.
    pub fn path(&self, role: &str) -> Result<&PathBuf> {
        self.paths
            .get(role)
            .ok_or_else(|| Error::Engine(format!("{} has no \"{role}\" file", self.model_id)))
    }
}

#[derive(Debug, Clone, Default)]
pub struct TranscribeOptions {
    /// ISO 639-1 code, or `None` to let the model detect the language.
    pub language: Option<String>,
    /// Words to favour (the user's dictionary), where the model supports it.
    pub hotwords: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Transcript {
    pub text: String,
    /// The language the model reports, when it does.
    pub language: Option<String>,
}

pub trait Transcriber: Send {
    /// Loads the model described by `spec`, replacing any model already loaded.
    fn load(&mut self, spec: &LoadSpec) -> Result<()>;

    /// Catalog id of the loaded model, if any.
    fn loaded_model(&self) -> Option<&str>;

    fn transcribe(&mut self, audio: &Audio, opts: &TranscribeOptions) -> Result<Transcript>;

    /// Frees the model's memory. Fine to call when nothing is loaded.
    fn unload(&mut self);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(channels: u16, rate: u32, samples: &[i16]) -> Vec<u8> {
        let data_len = (samples.len() * 2) as u32;
        let mut w = Vec::new();
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&(36 + data_len).to_le_bytes());
        w.extend_from_slice(b"WAVEfmt ");
        w.extend_from_slice(&16u32.to_le_bytes());
        w.extend_from_slice(&1u16.to_le_bytes());
        w.extend_from_slice(&channels.to_le_bytes());
        w.extend_from_slice(&rate.to_le_bytes());
        w.extend_from_slice(&(rate * 2 * channels as u32).to_le_bytes());
        w.extend_from_slice(&(2 * channels).to_le_bytes());
        w.extend_from_slice(&16u16.to_le_bytes());
        w.extend_from_slice(b"data");
        w.extend_from_slice(&data_len.to_le_bytes());
        for s in samples {
            w.extend_from_slice(&s.to_le_bytes());
        }
        w
    }

    #[test]
    fn reads_mono_pcm16() {
        let a = Audio::from_wav_bytes(&wav(1, 16_000, &[0, 16384, -16384, 32767])).unwrap();
        assert_eq!(a.sample_rate, 16_000);
        assert_eq!(a.samples.len(), 4);
        assert!((a.samples[1] - 0.5).abs() < 1e-4);
        assert!((a.samples[2] + 0.5).abs() < 1e-4);
        assert!(a.samples[3] < 1.0);
    }

    #[test]
    fn mixes_stereo_down_to_mono() {
        let a = Audio::from_wav_bytes(&wav(2, 8_000, &[16384, 0, 16384, 16384])).unwrap();
        assert_eq!(a.samples.len(), 2);
        assert!((a.samples[0] - 0.25).abs() < 1e-4);
        assert!((a.samples[1] - 0.5).abs() < 1e-4);
    }

    #[test]
    fn duration_is_computed_from_the_rate() {
        let a = Audio { sample_rate: 16_000, samples: vec![0.0; 8_000] };
        assert_eq!(a.duration_ms(), 500);
        assert_eq!(Audio { sample_rate: 0, samples: vec![0.0; 10] }.duration_ms(), 0);
    }

    #[test]
    fn refuses_what_it_cannot_read() {
        assert!(Audio::from_wav_bytes(b"nope").is_err());
        assert!(Audio::from_wav_bytes(&[]).is_err());
        // 8-bit
        let mut w = wav(1, 16_000, &[0, 0]);
        w[34] = 8;
        assert!(Audio::from_wav_bytes(&w).is_err());
        // no data chunk
        assert!(Audio::from_wav_bytes(&wav(1, 16_000, &[1])[..36]).is_err());
    }

    #[test]
    fn a_truncated_data_chunk_yields_only_the_whole_samples_that_are_there() {
        let mut w = wav(1, 16_000, &[100, 200, 300, 400]);
        w.truncate(w.len() - 3); // 8 bytes of audio become 5: two whole samples and half of a third
        let a = Audio::from_wav_bytes(&w).unwrap();
        assert_eq!(a.samples.len(), 2);
    }
}
