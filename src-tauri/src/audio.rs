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

    /// Stop recording and return a WAV-encoded buffer (mono, 16-bit PCM).
    pub fn finish(self) -> Result<Vec<u8>> {
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

        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: self.sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };

        let mut buf = Cursor::new(Vec::<u8>::new());
        {
            let mut writer = hound::WavWriter::new(&mut buf, spec).context("creating WAV writer")?;
            for s in mono {
                writer.write_sample(s)?;
            }
            writer.finalize()?;
        }
        Ok(buf.into_inner())
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
