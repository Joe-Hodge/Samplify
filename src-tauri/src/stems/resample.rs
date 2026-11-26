//! Device rate ↔ the model's 44.1 kHz.
//!
//! Pinned to rubato 0.16 deliberately: 4.0 replaced the planar `SincFixedIn` API
//! with an `Async`/`audioadapter::Adapter` design that adds a transitive
//! dependency and a lot of surface for what is a fixed 48k↔44.1k conversion.

use anyhow::Result;
use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};

/// Resample planar per-channel buffers. Returns a clone when the rates already
/// match, so callers can invoke this unconditionally.
pub fn resample(input: &[Vec<f32>], from_hz: u32, to_hz: u32) -> Result<Vec<Vec<f32>>> {
    if from_hz == to_hz || input.is_empty() {
        return Ok(input.to_vec());
    }
    let channels = input.len();
    let in_len = input[0].len();
    if in_len == 0 {
        return Ok(vec![Vec::new(); channels]);
    }

    let params = SincInterpolationParameters {
        sinc_len: 256,
        f_cutoff: 0.95,
        interpolation: SincInterpolationType::Linear,
        oversampling_factor: 256,
        window: WindowFunction::BlackmanHarris2,
    };
    let ratio = to_hz as f64 / from_hz as f64;
    let mut r = SincFixedIn::<f32>::new(ratio, 2.0, params, in_len, channels)?;
    let out = r.process(input, None)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_is_a_noop_at_equal_rates() {
        let ch = vec![vec![0.1, 0.2, 0.3, 0.4], vec![0.5, 0.6, 0.7, 0.8]];
        let out = resample(&ch, 44_100, 44_100).unwrap();
        assert_eq!(out, ch, "equal rates must not touch the samples at all");
    }

    #[test]
    fn resample_48k_to_44k1_scales_length_and_preserves_channels() {
        let n = 48_000; // 1 second
        let ch: Vec<Vec<f32>> = (0..2)
            .map(|c| (0..n).map(|i| ((i as f32 / 48_000.0) * 440.0 * std::f32::consts::TAU + c as f32).sin() * 0.5).collect())
            .collect();

        let out = resample(&ch, 48_000, 44_100).unwrap();

        assert_eq!(out.len(), 2, "channel count preserved");
        let want = 44_100f64;
        let got = out[0].len() as f64;
        assert!((got - want).abs() / want < 0.01, "expected ~44100 samples, got {got}");
        assert_eq!(out[0].len(), out[1].len(), "channels must stay aligned");
        assert!(out[0].iter().all(|v| v.is_finite()), "no NaN/inf in output");
    }

    #[test]
    fn resample_roundtrip_is_phase_aligned_and_only_loses_its_tail() {
        // The property that actually matters: a stem must line up sample-for-sample
        // with the mix it came from, or layering it in the Playground comb-filters.
        // rubato compensates the sinc filter delay, so an impulse survives a
        // 48k->44.1k->48k roundtrip at the SAME index. What it cannot produce is the
        // last ~sinc_len samples of tail, so the buffer comes back slightly shorter —
        // separate_to_files pads that back to keep stems exactly as long as the source.
        let n = 48_000;
        let mut ch = vec![vec![0.0f32; n], vec![0.0f32; n]];
        ch[0][10_000] = 1.0;
        ch[1][10_000] = 1.0;

        let down = resample(&ch, 48_000, 44_100).unwrap();
        let up = resample(&down, 44_100, 48_000).unwrap();

        let peak = |v: &[f32]| v.iter().enumerate()
            .max_by(|a, b| a.1.abs().partial_cmp(&b.1.abs()).unwrap()).map(|(i, _)| i).unwrap();

        // Downsampling moves the impulse by exactly the rate ratio.
        let want_down = (10_000f64 * 44_100.0 / 48_000.0).round() as i64;
        assert!((peak(&down[0]) as i64 - want_down).abs() <= 2,
            "downsampled impulse at {}, expected ~{want_down}", peak(&down[0]));

        // And the roundtrip puts it back exactly where it started.
        assert!((peak(&up[0]) as i64 - 10_000).abs() <= 2,
            "roundtrip impulse landed at {}, expected 10000 — a phase shift here would \
             make every stem drift against the mix", peak(&up[0]));

        // Tail loss only, never growth, and bounded by the filter length.
        let lost = n as i64 - up[0].len() as i64;
        assert!(lost >= 0 && lost <= 2 * 256 + 64, "unexpected tail loss of {lost} samples");
    }
}
