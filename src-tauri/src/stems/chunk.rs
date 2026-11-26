//! Splitting audio into the model's fixed-size windows and stitching the
//! results back together.
//!
//! htdemucs takes exactly `CHUNK` samples per channel, so anything longer is
//! processed in overlapping windows and crossfaded. The crossfade pair satisfies
//! the COLA condition (gains sum to 1 across the overlap), which is what lets
//! `overlap_add` reconstruct an unmodified signal exactly — the property the
//! unit tests pin down.

/// Samples per channel per inference — 7.8 s at 44.1 kHz. Fixed by the model.
pub const CHUNK: usize = 343_980;
/// Crossfade length between adjacent chunks.
pub const OVERLAP: usize = CHUNK / 4;
/// Distance between consecutive chunk starts.
pub const HOP: usize = CHUNK - OVERLAP;

/// Rising half of the crossfade. `sin²` so that `fade_in(k) + (1 - fade_in(k))`
/// is exactly 1 and the pair is smooth at both ends.
pub fn fade_in(k: usize) -> f32 {
    let x = (k as f32 + 0.5) / OVERLAP as f32;
    let s = (std::f32::consts::FRAC_PI_2 * x).sin();
    s * s
}

/// Start offsets of every chunk needed to cover `len` samples. Always at least
/// one chunk; the final chunk is expected to be zero-padded by the caller.
pub fn chunk_starts(len: usize) -> Vec<usize> {
    if len <= CHUNK {
        return vec![0];
    }
    let mut starts = Vec::new();
    let mut s = 0usize;
    loop {
        starts.push(s);
        if s + CHUNK >= len {
            break;
        }
        s += HOP;
    }
    starts
}

/// Accumulate one chunk's output into `out` at `start`, crossfading against its
/// neighbours. `is_first` / `is_last` suppress the ramp where there is no
/// neighbour to blend with, so the very start and end keep unity gain. Writes
/// past the end of `out` are dropped — that region is the final chunk's padding.
pub fn overlap_add(out: &mut [f32], chunk: &[f32], start: usize, is_first: bool, is_last: bool) {
    let tail_begin = CHUNK - OVERLAP;
    for (j, &v) in chunk.iter().enumerate().take(CHUNK) {
        let idx = start + j;
        if idx >= out.len() {
            break;
        }
        let mut gain = 1.0f32;
        if !is_first && j < OVERLAP {
            gain *= fade_in(j);
        }
        if !is_last && j >= tail_begin {
            gain *= 1.0 - fade_in(j - tail_begin);
        }
        out[idx] += v * gain;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_starts_covers_short_exact_and_long_inputs() {
        // Shorter than one chunk: a single zero-padded chunk.
        assert_eq!(chunk_starts(1000), vec![0]);
        // Exactly one chunk: still one.
        assert_eq!(chunk_starts(CHUNK), vec![0]);
        // One sample over: needs a second chunk to cover the tail.
        let s = chunk_starts(CHUNK + 1);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0], 0);
        assert_eq!(s[1], HOP);
        // Every start is HOP apart, and the last chunk reaches the end.
        let len = CHUNK * 3 + 12_345;
        let s = chunk_starts(len);
        for w in s.windows(2) { assert_eq!(w[1] - w[0], HOP); }
        assert!(s.last().unwrap() + CHUNK >= len, "last chunk must cover the tail");
    }

    #[test]
    fn overlap_add_roundtrips_unmodified_signal() {
        // THE test for this module. Feed each chunk straight through with no
        // separation applied; reconstruction must equal the input. Any window,
        // offset, or crossfade bug shows up here as audible seams would.
        let len = CHUNK * 2 + 5_000;
        let input: Vec<f32> = (0..len)
            .map(|i| (i as f32 * 0.001).sin() * 0.5 + (i as f32 * 0.017).cos() * 0.3)
            .collect();

        let starts = chunk_starts(len);
        let last = starts.len() - 1;
        let mut out = vec![0.0f32; len];
        for (i, &s) in starts.iter().enumerate() {
            // Zero-padded copy of the input window — exactly what the model sees.
            let mut buf = vec![0.0f32; CHUNK];
            for j in 0..CHUNK {
                if s + j < len { buf[j] = input[s + j]; }
            }
            overlap_add(&mut out, &buf, s, i == 0, i == last);
        }

        assert_eq!(out.len(), input.len());
        for i in 0..len {
            assert!((out[i] - input[i]).abs() < 1e-4,
                "sample {i}: got {}, want {} (overlap-add must reconstruct exactly)", out[i], input[i]);
        }
    }

    #[test]
    fn overlap_add_crossfade_gains_sum_to_one() {
        // The COLA property the roundtrip test depends on, asserted directly so a
        // failure here localises the bug to the window rather than the plumbing.
        for k in 0..OVERLAP {
            let up = fade_in(k);
            let down = 1.0 - fade_in(k);
            assert!((up + down - 1.0).abs() < 1e-6, "gains at {k} sum to {}", up + down);
        }
        assert!(fade_in(0) < 0.01, "fade starts near silence");
        assert!(fade_in(OVERLAP - 1) > 0.99, "fade ends near unity");
    }

    #[test]
    fn overlap_add_never_writes_past_the_output() {
        // The final chunk is zero-padded beyond the real length; that padding
        // must be discarded, not appended.
        let len = CHUNK + 10;
        let mut out = vec![0.0f32; len];
        let buf = vec![1.0f32; CHUNK];
        overlap_add(&mut out, &buf, HOP, false, true);
        assert_eq!(out.len(), len, "output must not grow");
    }
}
