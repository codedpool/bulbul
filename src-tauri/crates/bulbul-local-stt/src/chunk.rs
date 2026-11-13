// SPDX-License-Identifier: GPL-3.0-only
// Copyright (c) 2026 Romanch Roshan Singh

//! Splitting long recordings into pieces an engine can take.
//!
//! Several model families only handle a limited stretch of audio at a time
//! (Whisper: about 30 seconds). A long dictation is cut into pieces, and each
//! cut is placed at the *quietest* moment near the limit rather than at an
//! arbitrary sample, so words are rarely split in half.

use crate::catalog::ModelKind;

/// Frame length used to measure how quiet a stretch of audio is.
const FRAME_MS: u32 = 20;
/// How far back from the limit a quiet spot is looked for.
const SEARCH_SECONDS: f32 = 5.0;
/// A leftover tail shorter than this is folded into the previous piece
/// instead of becoming a tiny piece of its own (engines invent words from
/// nearly-empty audio).
const MIN_TAIL_SECONDS: f32 = 0.6;

/// The longest piece, in seconds, to hand to a model of this family.
pub fn max_chunk_seconds(kind: ModelKind) -> f32 {
    match kind {
        // Hard ~30 s window; leave room for the folded-in tail.
        ModelKind::Whisper => 28.0,
        ModelKind::Moonshine | ModelKind::Qwen3Asr | ModelKind::OmnilingualCtc => 28.0,
        // Streaming-style architectures cope with much longer audio.
        ModelKind::Transducer | ModelKind::NemoCtc => 60.0,
    }
}

/// Splits `samples` into pieces of at most about `max_seconds`, cutting at
/// quiet spots. The pieces are contiguous and together cover every sample.
pub fn split_at_quiet_points(samples: &[f32], sample_rate: u32, max_seconds: f32) -> Vec<&[f32]> {
    if samples.is_empty() || sample_rate == 0 {
        return Vec::new();
    }
    let max = ((max_seconds * sample_rate as f32) as usize).max(1);
    if samples.len() <= max {
        return vec![samples];
    }
    let frame = ((sample_rate * FRAME_MS / 1000) as usize).max(1);
    let search = (SEARCH_SECONDS * sample_rate as f32) as usize;
    let min_tail = (MIN_TAIL_SECONDS * sample_rate as f32) as usize;

    // Work in index ranges, then slice once at the end.
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut start = 0usize;
    while samples.len() - start > max {
        let limit = start + max;
        // Look back from the limit, but never closer to `start` than half a
        // piece, so every step makes real progress.
        let from = limit.saturating_sub(search).max(start + max / 2);
        let cut = quietest_point(&samples[from..limit], frame) + from;
        ranges.push((start, cut));
        start = cut;
    }
    if samples.len() - start < min_tail && !ranges.is_empty() {
        // Fold a tiny remainder into the last piece.
        let (from, _) = ranges.pop().unwrap();
        ranges.push((from, samples.len()));
    } else {
        ranges.push((start, samples.len()));
    }
    ranges.into_iter().map(|(a, b)| &samples[a..b]).collect()
}

/// Index (relative to `window`) of the start of the quietest frame, measured
/// by mean absolute amplitude. Ties go to the later frame, keeping pieces long.
fn quietest_point(window: &[f32], frame: usize) -> usize {
    let mut best = window.len() / 2;
    let mut best_energy = f32::MAX;
    let mut i = 0;
    while i + frame <= window.len() {
        let energy = window[i..i + frame].iter().map(|s| s.abs()).sum::<f32>() / frame as f32;
        if energy <= best_energy {
            best_energy = energy;
            best = i + frame / 2;
        }
        i += frame;
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 16_000;

    /// Loud "speech" with a quiet gap of `gap_ms` starting at `gap_at` seconds.
    fn audio(seconds: f32, gaps: &[(f32, u32)]) -> Vec<f32> {
        let n = (seconds * RATE as f32) as usize;
        let mut v: Vec<f32> = (0..n).map(|i| 0.5 * ((i as f32) * 0.05).sin()).collect();
        for (at, ms) in gaps {
            let s = (*at * RATE as f32) as usize;
            let e = (s + (*ms as usize * RATE as usize / 1000)).min(n);
            for x in &mut v[s..e] {
                *x = 0.0;
            }
        }
        v
    }

    fn rejoin(pieces: &[&[f32]]) -> Vec<f32> {
        pieces.iter().flat_map(|p| p.iter().copied()).collect()
    }

    #[test]
    fn short_audio_is_left_whole() {
        let a = audio(10.0, &[]);
        let p = split_at_quiet_points(&a, RATE, 28.0);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].len(), a.len());
    }

    #[test]
    fn nothing_in_nothing_out() {
        assert!(split_at_quiet_points(&[], RATE, 28.0).is_empty());
        assert!(split_at_quiet_points(&[0.1; 10], 0, 28.0).is_empty());
    }

    #[test]
    fn long_audio_is_split_into_contiguous_pieces_that_cover_everything() {
        let a = audio(100.0, &[]);
        let p = split_at_quiet_points(&a, RATE, 28.0);
        assert!(p.len() >= 4);
        assert_eq!(rejoin(&p), a, "no sample lost, repeated or reordered");
        let max = (28.0 * RATE as f32) as usize;
        let slack = (MIN_TAIL_SECONDS * RATE as f32) as usize;
        for piece in &p {
            assert!(piece.len() <= max + slack, "piece of {} samples", piece.len());
            assert!(!piece.is_empty());
        }
    }

    #[test]
    fn cuts_land_in_a_quiet_gap_near_the_limit() {
        // A 300 ms pause at 26 s, inside the 5 s look-back before the 28 s limit.
        let a = audio(40.0, &[(26.0, 300)]);
        let p = split_at_quiet_points(&a, RATE, 28.0);
        assert_eq!(p.len(), 2);
        let cut = p[0].len() as f32 / RATE as f32;
        assert!((26.0..26.31).contains(&cut), "cut at {cut} s should be inside the pause");
    }

    #[test]
    fn a_tiny_tail_is_folded_into_the_previous_piece() {
        // Constant-level audio has no quiet spot, so cuts fall at the limit and
        // the leftovers are predictable.
        let flat = |secs: f32| vec![0.3f32; (secs * RATE as f32) as usize];

        // 28.2 s: the 0.2 s leftover would be a useless piece of its own.
        let a = flat(28.2);
        let p = split_at_quiet_points(&a, RATE, 28.0);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].len(), a.len());

        // 56.3 s: after two cuts a 0.3 s remainder is folded back in.
        let a = flat(56.3);
        let p = split_at_quiet_points(&a, RATE, 28.0);
        assert_eq!(p.len(), 2);
        assert_eq!(rejoin(&p), a);

        // 57.0 s: a 1 s remainder is long enough to stand alone.
        let a = flat(57.0);
        let p = split_at_quiet_points(&a, RATE, 28.0);
        assert_eq!(p.len(), 3);
        assert_eq!(rejoin(&p), a);
    }

    #[test]
    fn works_at_other_sample_rates() {
        let n = 44_100 * 70;
        let a: Vec<f32> = (0..n).map(|i| 0.3 * ((i as f32) * 0.01).sin()).collect();
        let p = split_at_quiet_points(&a, 44_100, 28.0);
        assert_eq!(rejoin(&p), a);
        assert!(p.len() >= 3);
    }

    #[test]
    fn limits_depend_on_the_model_family() {
        assert!(max_chunk_seconds(ModelKind::Whisper) < 30.0, "stays under Whisper's 30 s window");
        assert!(max_chunk_seconds(ModelKind::Transducer) > max_chunk_seconds(ModelKind::Whisper));
    }
}
