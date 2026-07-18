use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};
use parking_lot::Mutex;
use std::io::Cursor;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

/// Shared state between the cpal input callback and the orchestrator.
/// The callback always fires while the stream is playing, but only
/// pushes samples when `recording` is set — that way starting and
/// stopping a dictation is just a flag flip plus a fast WASAPI
/// `Start`/`Stop`, with no per-press device re-open.
struct SessionState {
    recording: AtomicBool,
    samples: Mutex<Vec<i16>>,
}

/// Module-singleton persistent audio. Built once on first use (or at
/// startup via `prewarm`); the stream stays alive for the lifetime of
/// the process. Between dictations the stream is paused, so the
/// Windows microphone-in-use indicator should only show while a
/// recording is actually in flight.
struct PersistentAudio {
    stream: Stream,
    state: Arc<SessionState>,
    sample_rate: u32,
    channels: u16,
}

// SAFETY: cpal's Windows (WASAPI) Stream is internally thread-safe — the
// audio callback runs on a system audio thread, and play/pause are
// COM-protected. Holding it behind a Mutex<Option<...>> gives us
// exclusive access on the control side. We need the manual Send impl
// because cpal::Stream is platform-conditionally Send and the type
// system can't see the cfg.
unsafe impl Send for PersistentAudio {}

static PERSISTENT: OnceLock<Mutex<Option<PersistentAudio>>> = OnceLock::new();

fn slot() -> &'static Mutex<Option<PersistentAudio>> {
    PERSISTENT.get_or_init(|| Mutex::new(None))
}

/// Pre-warm the audio stream during app startup so the first dictation
/// doesn't pay the WASAPI initialisation tax (~300–700 ms on observed
/// hardware). Idempotent. Failures are logged but not fatal — we'll
/// retry lazily on the first press.
pub fn prewarm() {
    let t0 = Instant::now();
    match ensure_built() {
        Ok(_) => tracing::info!(
            "audio prewarm complete in {}ms",
            t0.elapsed().as_millis()
        ),
        Err(e) => tracing::warn!("audio prewarm failed (will retry on first press): {e:#}"),
    }
}

fn ensure_built() -> Result<()> {
    let mut g = slot().lock();
    if g.is_some() {
        return Ok(());
    }
    *g = Some(PersistentAudio::build()?);
    Ok(())
}

impl PersistentAudio {
    fn build() -> Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .context("no default input device — check Windows sound settings")?;
        let name = device.name().unwrap_or_else(|_| "<unknown>".to_string());
        let config = device
            .default_input_config()
            .context("could not query default input config")?;
        tracing::info!(
            "audio: building persistent stream from device {:?} @ {} Hz, {} channels, {:?}",
            name,
            config.sample_rate().0,
            config.channels(),
            config.sample_format()
        );

        let sample_rate = config.sample_rate().0;
        let channels = config.channels();
        let stream_config: StreamConfig = config.clone().into();
        let state = Arc::new(SessionState {
            recording: AtomicBool::new(false),
            samples: Mutex::new(Vec::with_capacity((sample_rate as usize) * 4)),
        });

        let err_fn = |err| tracing::error!("audio stream error: {err}");
        let stream = match config.sample_format() {
            SampleFormat::F32 => {
                let st = state.clone();
                device.build_input_stream(
                    &stream_config,
                    move |data: &[f32], _| {
                        if !st.recording.load(Ordering::Acquire) {
                            return;
                        }
                        let mut g = st.samples.lock();
                        g.reserve(data.len());
                        for &s in data {
                            let clamped = s.clamp(-1.0, 1.0);
                            g.push((clamped * i16::MAX as f32) as i16);
                        }
                    },
                    err_fn,
                    None,
                )?
            }
            SampleFormat::I16 => {
                let st = state.clone();
                device.build_input_stream(
                    &stream_config,
                    move |data: &[i16], _| {
                        if !st.recording.load(Ordering::Acquire) {
                            return;
                        }
                        st.samples.lock().extend_from_slice(data);
                    },
                    err_fn,
                    None,
                )?
            }
            SampleFormat::U16 => {
                let st = state.clone();
                device.build_input_stream(
                    &stream_config,
                    move |data: &[u16], _| {
                        if !st.recording.load(Ordering::Acquire) {
                            return;
                        }
                        let mut g = st.samples.lock();
                        g.reserve(data.len());
                        for &s in data {
                            // u16 silence sits at 32768, not i16::MAX (32767).
                            g.push((s as i32 - 32768) as i16);
                        }
                    },
                    err_fn,
                    None,
                )?
            }
            fmt => return Err(anyhow!("unsupported sample format: {fmt:?}")),
        };

        // Initial play→pause warms WASAPI fully — subsequent play() is
        // then just IAudioClient::Start (the fast path), not the full
        // create-and-negotiate flow that dominated press→listening.
        stream
            .play()
            .context("warming the persistent stream (initial play)")?;
        stream
            .pause()
            .context("warming the persistent stream (initial pause)")?;
        Ok(Self {
            stream,
            state,
            sample_rate,
            channels,
        })
    }
}

/// Public recorder facade. Holds a clone of the persistent state and
/// the metadata needed to encode the final WAV; the cpal stream itself
/// lives in the static slot and is never dropped between dictations.
pub struct Recorder {
    state: Arc<SessionState>,
    sample_rate: u32,
    channels: u16,
}

impl Recorder {
    pub fn start() -> Result<Self> {
        let t0 = Instant::now();
        ensure_built()?;
        let g = slot().lock();
        let pa = g.as_ref().expect("persistent audio present after ensure_built");
        // Reset session state before unpausing so the callback never sees
        // a stale buffer on its first invocation of the new session.
        pa.state.samples.lock().clear();
        pa.state.recording.store(true, Ordering::Release);
        let play_started = Instant::now();
        pa.stream.play().context("stream.play")?;
        tracing::debug!(
            "audio: session start — ensure_built+lock={}µs stream.play={}µs",
            play_started.duration_since(t0).as_micros(),
            play_started.elapsed().as_micros(),
        );
        Ok(Self {
            state: pa.state.clone(),
            sample_rate: pa.sample_rate,
            channels: pa.channels,
        })
    }

    /// Stop recording and return WAV bytes plus signal metrics.
    pub fn finish(self) -> Result<RecordingResult> {
        // Stop the cpal callback from accepting more samples, then pause
        // the WASAPI stream so the mic-indicator goes back to "off"
        // between dictations. We drain the session buffer afterwards
        // because the callback may still be mid-write when we flip the
        // recording flag.
        {
            let g = slot().lock();
            if let Some(pa) = g.as_ref() {
                pa.state.recording.store(false, Ordering::Release);
                if let Err(e) = pa.stream.pause() {
                    tracing::warn!("audio: stream.pause failed: {e}");
                }
            }
        }
        let samples = std::mem::take(&mut *self.state.samples.lock());

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

        // Capture the *pre-normalization* peak AND RMS. The silence gate
        // upstream must look at both pre-AGC — otherwise AGC inflates the
        // noise floor and the gate becomes useless. We want two metrics
        // because peak alone is fooled by a single click in an otherwise
        // silent room (a mouse-click spike can lift peak above -55 dBFS
        // while average energy stays at room-tone levels), and RMS alone
        // would be fooled by a sustained drone. Speech reliably has both
        // a high peak AND a meaningful average — only ambient noise has
        // one without the other.
        let peak_dbfs = compute_peak_dbfs(&mono);
        let rms_dbfs = compute_rms_dbfs(&mono);
        let max_window_rms_dbfs = compute_max_window_rms_dbfs(&mono, self.sample_rate);
        let seconds = if self.sample_rate == 0 {
            0.0
        } else {
            mono.len() as f32 / self.sample_rate as f32
        };

        // Strip sub-speech rumble and DC before AGC + upload. Windows'
        // microphone enhancement already high-passes the raw capture;
        // ALSA/PulseAudio (Linux) and CoreAudio (macOS) hand us the raw
        // signal, so low-frequency noise (fans, desk thumps, HVAC, DC
        // offset) otherwise survives all the way to Whisper and drags
        // accuracy down. 80 Hz sits below the speech fundamental, so
        // nothing intelligible is removed. This is why the same mic
        // transcribes cleanly on Windows but noisier on Linux/macOS.
        let mono = high_pass(mono, self.sample_rate, HIGH_PASS_HZ);

        // Loudness-target AGC. Whisper mistranscribes low-amplitude input,
        // so we lift quiet speech to a healthy level. A pure peak-normalize
        // is fooled by a single click that pins the peak while the actual
        // speech stays quiet — common on raw Linux/macOS capture — so we
        // also target the loudest 30 ms window's RMS and take whichever
        // asks for MORE gain. The result is always >= the old peak-only
        // gain, so the already-clean Windows path is never boosted less
        // than before; a -1 dBFS peak ceiling guards against clipping and
        // MAX_GAIN stops near-silence exploding into hiss.
        let (normalized, applied_gain) = normalize_loudness(
            mono,
            self.sample_rate,
            TARGET_PEAK_DBFS,
            TARGET_RMS_DBFS,
            PEAK_CEILING_DBFS,
            MAX_GAIN_LINEAR,
        );
        if applied_gain > 1.01 {
            tracing::info!(
                "AGC boost: peak {:.1} dBFS -> ~{:.1} dBFS (gain {:.1}x)",
                peak_dbfs,
                peak_dbfs + 20.0 * applied_gain.log10(),
                applied_gain
            );
        }

        // Resample to 16 kHz — Whisper's internal rate, so we're not throwing
        // away anything by doing it locally with a proper anti-alias filter
        // instead of uploading the full-rate WAV and letting Whisper downsample
        // on its end. Cuts the upload payload to ~1/3 on a 48 kHz mic. Mics
        // with other native rates pass through unchanged.
        let (final_samples, out_rate) = if self.sample_rate == 48000 {
            (decimate_48k_to_16k(&normalized), 16000u32)
        } else {
            (normalized, self.sample_rate)
        };

        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: out_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };

        let mut buf = Cursor::new(Vec::<u8>::new());
        {
            let mut writer = hound::WavWriter::new(&mut buf, spec).context("creating WAV writer")?;
            for s in final_samples {
                writer.write_sample(s)?;
            }
            writer.finalize()?;
        }
        Ok(RecordingResult {
            wav: buf.into_inner(),
            peak_dbfs,
            rms_dbfs,
            max_window_rms_dbfs,
            seconds,
        })
    }

    pub fn captured_seconds(&self) -> f32 {
        let len = self.state.samples.lock().len() as f32;
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
    pub rms_dbfs: f32,
    /// RMS of the loudest 30 ms window in the clip. This is what the
    /// silence gate should look at — clip-wide RMS gets dragged down by
    /// leading/trailing dead air, which penalises slow speakers and
    /// distant talkers even when their actual speech is clearly audible.
    pub max_window_rms_dbfs: f32,
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

/// Average energy of the clip in dBFS. Computed in f64 because squaring
/// i16 samples and summing across a multi-second recording can overflow
/// f32's mantissa precision near full-scale clips. Returns -120 dBFS for
/// truly empty or pure-zero buffers as a safe floor.
fn compute_rms_dbfs(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return -120.0;
    }
    let scale = i16::MAX as f64;
    let sum_sq: f64 = samples
        .iter()
        .map(|&s| {
            let f = s as f64 / scale;
            f * f
        })
        .sum();
    let rms = (sum_sq / samples.len() as f64).sqrt();
    if rms <= 0.0 {
        return -120.0;
    }
    (20.0 * rms.log10()) as f32
}

/// RMS in dBFS of the loudest 30 ms window in the clip. Sliding hop is
/// 15 ms (50 % overlap) so a voiced moment never falls between cracks.
///
/// We need this because the silence gate that runs over the recording
/// can't use the clip-wide RMS — a user who speaks softly from across
/// the room and pauses between words has a clip whose average energy is
/// dominated by silence, even when the actual speech is plainly audible
/// in any individual window. The maximum window RMS reflects the
/// *loudest moment* in the clip, which is what we actually care about
/// when deciding "is there any speech here at all?".
///
/// Returns -120 dBFS for empty / zero buffers so the caller can compare
/// against a fixed threshold without special-casing.
fn compute_max_window_rms_dbfs(samples: &[i16], sample_rate: u32) -> f32 {
    if samples.is_empty() || sample_rate == 0 {
        return -120.0;
    }
    let window = ((sample_rate as usize) * 30 / 1000).max(1);
    // Short clip: one window is the whole thing — fall back to clip RMS.
    if samples.len() <= window {
        return compute_rms_dbfs(samples);
    }
    let hop = (window / 2).max(1);
    let scale = i16::MAX as f64;
    let mut best_db = f32::NEG_INFINITY;
    let mut start = 0;
    while start + window <= samples.len() {
        let sum_sq: f64 = samples[start..start + window]
            .iter()
            .map(|&s| {
                let f = s as f64 / scale;
                f * f
            })
            .sum();
        let rms = (sum_sq / window as f64).sqrt();
        if rms > 0.0 {
            let dbfs = (20.0 * rms.log10()) as f32;
            if dbfs > best_db {
                best_db = dbfs;
            }
        }
        start += hop;
    }
    if best_db == f32::NEG_INFINITY {
        -120.0
    } else {
        best_db
    }
}

/// The floor target: bring the peak to just below 0 dBFS so we never clip
/// on the loudest sample after rounding. -3 dBFS = ~0.708 of full scale.
/// This is the old peak-normalize target, kept as a lower bound so the
/// Windows path is never boosted less than it was before this change.
const TARGET_PEAK_DBFS: f32 = -3.0;

/// The loudness target: bring the loudest 30 ms window's RMS up to here.
/// Speech RMS typically sits 12–16 dB below its peak, so -16 dBFS RMS
/// pairs with a peak comfortably under the -1 dBFS ceiling. This is what
/// actually rescues quiet-but-clicky Linux/macOS capture, where the peak
/// target alone under-boosts because a stray transient pins the peak.
const TARGET_RMS_DBFS: f32 = -16.0;

/// Never let the post-gain peak exceed this — headroom against clipping
/// when the RMS target demands aggressive gain.
const PEAK_CEILING_DBFS: f32 = -1.0;

/// High-pass corner. Below the speech fundamental (~85 Hz for a low male
/// voice), so it removes only rumble/DC, never intelligible content.
const HIGH_PASS_HZ: f32 = 80.0;

/// Hard ceiling on how much we'll amplify. +29.5 dB lifts a -50 dBFS
/// whisper to ~-20 dBFS — comfortably above Whisper's confidence floor —
/// without letting near-silent buffers explode into hiss.
const MAX_GAIN_LINEAR: f32 = 30.0;

/// Loudness-target auto-gain. Computes the gain that would hit the peak
/// target AND the gain that would hit the loudest-window RMS target, then
/// applies whichever is LARGER — clamped by a peak ceiling (no clipping)
/// and MAX_GAIN (no exploding near-silence). Because the peak-target gain
/// is always one of the candidates and we take the max, the result is
/// never smaller than the previous peak-only normalize, so the clean
/// Windows path can't regress. Only ever amplifies (gain >= 1.0).
fn normalize_loudness(
    mut samples: Vec<i16>,
    sample_rate: u32,
    target_peak_dbfs: f32,
    target_rms_dbfs: f32,
    ceiling_dbfs: f32,
    max_gain: f32,
) -> (Vec<i16>, f32) {
    let peak = samples
        .iter()
        .map(|&s| s.unsigned_abs() as u32)
        .max()
        .unwrap_or(0);
    if peak == 0 {
        return (samples, 1.0);
    }
    let full = i16::MAX as f32;
    let peak_f = peak as f32;

    // Gain to reach the peak target (the floor — old behaviour).
    let gain_peak = 10f32.powf(target_peak_dbfs / 20.0) * full / peak_f;

    // Gain to bring the loudest 30 ms window's RMS up to target. Uses the
    // windowed RMS (not clip-wide) so leading/trailing silence doesn't
    // drag it down and over-boost.
    let win_rms_dbfs = compute_max_window_rms_dbfs(&samples, sample_rate);
    let gain_rms = if win_rms_dbfs <= -119.0 {
        1.0
    } else {
        10f32.powf((target_rms_dbfs - win_rms_dbfs) / 20.0)
    };

    // Take the more aggressive of the two, but never push the peak past
    // the ceiling, and never exceed MAX_GAIN.
    let ceiling_gain = 10f32.powf(ceiling_dbfs / 20.0) * full / peak_f;
    let gain = gain_peak
        .max(gain_rms)
        .min(ceiling_gain)
        .clamp(1.0, max_gain);

    if (gain - 1.0).abs() < 0.01 {
        return (samples, 1.0);
    }
    let max = full;
    let min = i16::MIN as f32;
    for s in samples.iter_mut() {
        let v = (*s as f32 * gain).clamp(min, max);
        *s = v as i16;
    }
    (samples, gain)
}

/// Second-order Butterworth high-pass (RBJ biquad, Q = 0.707) applied in
/// f32 to avoid quantisation. Removes DC offset and low-frequency rumble
/// that raw Linux/macOS capture carries and Windows' mic enhancement
/// already strips. Returns the input untouched for empty buffers or an
/// unusable sample rate.
fn high_pass(samples: Vec<i16>, sample_rate: u32, fc_hz: f32) -> Vec<i16> {
    if samples.is_empty() || sample_rate == 0 {
        return samples;
    }
    let sr = sample_rate as f32;
    // Guard: the corner must be below Nyquist or the filter is meaningless.
    if fc_hz <= 0.0 || fc_hz >= sr / 2.0 {
        return samples;
    }
    let w0 = 2.0 * std::f32::consts::PI * fc_hz / sr;
    let cos_w0 = w0.cos();
    let sin_w0 = w0.sin();
    let q = std::f32::consts::FRAC_1_SQRT_2; // 0.7071 → Butterworth
    let alpha = sin_w0 / (2.0 * q);

    let a0 = 1.0 + alpha;
    let b0 = ((1.0 + cos_w0) / 2.0) / a0;
    let b1 = (-(1.0 + cos_w0)) / a0;
    let b2 = ((1.0 + cos_w0) / 2.0) / a0;
    let a1 = (-2.0 * cos_w0) / a0;
    let a2 = (1.0 - alpha) / a0;

    let mut x1 = 0.0f32;
    let mut x2 = 0.0f32;
    let mut y1 = 0.0f32;
    let mut y2 = 0.0f32;
    let mut out = Vec::with_capacity(samples.len());
    for &s in &samples {
        let x0 = s as f32;
        let y0 = b0 * x0 + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2;
        x2 = x1;
        x1 = x0;
        y2 = y1;
        y1 = y0;
        out.push(y0.clamp(i16::MIN as f32, i16::MAX as f32) as i16);
    }
    out
}

/// 48 kHz → 16 kHz decimate-by-3 with a Hamming-windowed sinc anti-alias
/// filter (33 taps, cutoff ~8 kHz). Speech sits below 8 kHz so nothing
/// useful is dropped; the upload becomes 3× smaller, which is the whole
/// point. Whisper resamples to 16 kHz internally anyway — we're just doing
/// the conversion locally so we don't pay upload time on bytes Whisper
/// would discard.
fn decimate_48k_to_16k(input: &[i16]) -> Vec<i16> {
    let coeffs = lp_coefficients_48_to_16();
    let n_taps = coeffs.len();
    let center = n_taps / 2; // integer; coefficients are symmetric
    let out_len = input.len() / 3;
    let mut output = Vec::with_capacity(out_len);
    for k in 0..out_len {
        let center_in = (k * 3 + center) as isize;
        let mut sum = 0.0f32;
        for j in 0..n_taps {
            let idx = center_in + j as isize - center as isize;
            if idx >= 0 && (idx as usize) < input.len() {
                sum += input[idx as usize] as f32 * coeffs[j];
            }
        }
        let v = sum.clamp(i16::MIN as f32, i16::MAX as f32);
        output.push(v as i16);
    }
    output
}

/// Cached low-pass FIR coefficients for the 48 → 16 kHz decimator above.
/// Computed once on first dictation, reused forever. 33-tap Hamming-windowed
/// sinc at fc = 1/6 of the input rate (i.e. 8 kHz at 48 kHz input), with
/// DC gain normalised to 1.0.
fn lp_coefficients_48_to_16() -> &'static [f32; 33] {
    static COEFFS: OnceLock<[f32; 33]> = OnceLock::new();
    COEFFS.get_or_init(|| {
        const N: usize = 33;
        const FC: f32 = 1.0 / 6.0;
        let center = (N as f32 - 1.0) / 2.0;
        let mut c = [0.0f32; N];
        let mut sum = 0.0f32;
        for i in 0..N {
            let x = i as f32 - center;
            let sinc = if x.abs() < f32::EPSILON {
                2.0 * FC
            } else {
                (2.0 * std::f32::consts::PI * FC * x).sin() / (std::f32::consts::PI * x)
            };
            let w = 0.54
                - 0.46
                    * (2.0 * std::f32::consts::PI * i as f32 / (N as f32 - 1.0)).cos();
            c[i] = sinc * w;
            sum += c[i];
        }
        // Normalise DC gain to 1.0 so the resampled signal keeps the same
        // amplitude as the input — important because AGC has already set
        // the peak to TARGET_PEAK_DBFS and we don't want to undo that.
        for x in c.iter_mut() {
            *x /= sum;
        }
        c
    })
}
