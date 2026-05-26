use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};
use parking_lot::Mutex;
use std::io::Cursor;
use std::sync::Arc;

/// Captures audio from the system default input device.
/// Each call to start() resolves the default device fresh, so users can
/// switch headsets/mics between recordings without restarting the app.
pub struct Recorder {
    inner: Arc<Mutex<Inner>>,
    _stream: Stream,
    sample_rate: u32,
    channels: u16,
}

struct Inner {
    samples: Vec<i16>,
}

impl Recorder {
    pub fn start() -> Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .context("no default input device — check Windows sound settings")?;
        let name = device.name().unwrap_or_else(|_| "<unknown>".to_string());
        let config = device
            .default_input_config()
            .context("could not query default input config")?;
        tracing::info!(
            "recording from device {:?} @ {} Hz, {} channels, {:?}",
            name,
            config.sample_rate().0,
            config.channels(),
            config.sample_format()
        );

        let sample_rate = config.sample_rate().0;
        let channels = config.channels();
        let stream_config: StreamConfig = config.clone().into();
        let inner = Arc::new(Mutex::new(Inner {
            samples: Vec::with_capacity((sample_rate as usize) * 4),
        }));

        let err_fn = |err| tracing::error!("audio stream error: {err}");
        let stream = match config.sample_format() {
            SampleFormat::F32 => {
                let inner = inner.clone();
                device.build_input_stream(
                    &stream_config,
                    move |data: &[f32], _| {
                        let mut g = inner.lock();
                        g.samples.reserve(data.len());
                        for &s in data {
                            let clamped = s.clamp(-1.0, 1.0);
                            g.samples.push((clamped * i16::MAX as f32) as i16);
                        }
                    },
                    err_fn,
                    None,
                )?
            }
            SampleFormat::I16 => {
                let inner = inner.clone();
                device.build_input_stream(
                    &stream_config,
                    move |data: &[i16], _| {
                        let mut g = inner.lock();
                        g.samples.extend_from_slice(data);
                    },
                    err_fn,
                    None,
                )?
            }
            SampleFormat::U16 => {
                let inner = inner.clone();
                device.build_input_stream(
                    &stream_config,
                    move |data: &[u16], _| {
                        let mut g = inner.lock();
                        g.samples.reserve(data.len());
                        for &s in data {
                            g.samples.push((s as i32 - i16::MAX as i32) as i16);
                        }
                    },
                    err_fn,
                    None,
                )?
            }
            fmt => return Err(anyhow!("unsupported sample format: {fmt:?}")),
        };

        stream.play().context("starting input stream")?;

        Ok(Self {
            inner,
            _stream: stream,
            sample_rate,
            channels,
        })
    }

    /// Stop recording and return WAV bytes plus signal metrics.
    pub fn finish(self) -> Result<RecordingResult> {
        // Dropping _stream stops capture. Pull samples out.
        drop(self._stream);
        let samples = std::mem::take(&mut self.inner.lock().samples);

        // Downmix to mono if needed.
        let mono: Vec<i16> = if self.channels <= 1 {
            samples
        } else {
            let ch = self.channels as usize;
            samples
                .chunks_exact(ch)
                .map(|frame| {
                    let sum: i32 = frame.iter().map(|&s| s as i32).sum();
                    (sum / ch as i32) as i16
                })
                .collect()
        };

        // Capture the *pre-normalization* peak. The silence gate upstream
        // must look at this, not the post-AGC value — otherwise AGC just
        // inflates the noise floor and the gate becomes useless.
        let peak_dbfs = compute_peak_dbfs(&mono);
        let seconds = if self.sample_rate == 0 {
            0.0
        } else {
            mono.len() as f32 / self.sample_rate as f32
        };

        // Peak-normalize AGC. Whisper hallucinates on low-amplitude input;
        // boosting a whisper to normal-speech amplitude makes the model
        // treat it like any other clip. Gain is clamped so a near-silent
        // buffer can't multiply pure noise into garbage.
        let (normalized, applied_gain) = normalize_peak(mono, TARGET_PEAK_DBFS, MAX_GAIN_LINEAR);
        if applied_gain > 1.01 {
            tracing::info!(
                "AGC boost: peak {:.1} dBFS -> ~{:.1} dBFS (gain {:.1}x)",
                peak_dbfs,
                peak_dbfs + 20.0 * applied_gain.log10(),
                applied_gain
            );
        }

        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: self.sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };

        let mut buf = Cursor::new(Vec::<u8>::new());
        {
            let mut writer = hound::WavWriter::new(&mut buf, spec).context("creating WAV writer")?;
            for s in normalized {
                writer.write_sample(s)?;
            }
            writer.finalize()?;
        }
        Ok(RecordingResult {
            wav: buf.into_inner(),
            peak_dbfs,
            seconds,
        })
    }

    pub fn captured_seconds(&self) -> f32 {
        let len = self.inner.lock().samples.len() as f32;
        if self.sample_rate == 0 {
            0.0
        } else {
            len / (self.sample_rate as f32 * self.channels.max(1) as f32)
        }
    }
}

pub struct RecordingResult {
    pub wav: Vec<u8>,
    pub peak_dbfs: f32,
    pub seconds: f32,
}

fn compute_peak_dbfs(samples: &[i16]) -> f32 {
    let peak = samples
        .iter()
        .map(|&s| s.unsigned_abs() as u32)
        .max()
        .unwrap_or(0);
    if peak == 0 {
        return -120.0;
    }
    20.0 * (peak as f32 / i16::MAX as f32).log10()
}

/// Target the post-AGC peak just below 0 dBFS so we never clip on the
/// loudest sample after rounding. -3 dBFS = ~0.708 of full scale.
const TARGET_PEAK_DBFS: f32 = -3.0;

/// Hard ceiling on how much we'll amplify. +29.5 dB lifts a -50 dBFS
/// whisper to ~-20 dBFS — comfortably above Whisper's confidence floor —
/// without letting near-silent buffers explode into hiss.
const MAX_GAIN_LINEAR: f32 = 30.0;

fn normalize_peak(mut samples: Vec<i16>, target_dbfs: f32, max_gain: f32) -> (Vec<i16>, f32) {
    let peak = samples
        .iter()
        .map(|&s| s.unsigned_abs() as u32)
        .max()
        .unwrap_or(0);
    if peak == 0 {
        return (samples, 1.0);
    }
    let target_amp = 10f32.powf(target_dbfs / 20.0) * i16::MAX as f32;
    let gain = (target_amp / peak as f32).clamp(1.0, max_gain);
    if (gain - 1.0).abs() < 0.01 {
        return (samples, 1.0);
    }
    let max = i16::MAX as f32;
    let min = i16::MIN as f32;
    for s in samples.iter_mut() {
        let v = (*s as f32 * gain).clamp(min, max);
        *s = v as i16;
    }
    (samples, gain)
}
