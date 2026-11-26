//! Mixing separated stems back together at chosen gains.
//!
//! This is what makes the four stems more than four files: a karaoke mix is
//! `vocals: 0`, "drums + bass" is `other: 0, vocals: 0`, and any fader position
//! in between is a real mix rather than a mute. Pure sample arithmetic on WAVs
//! the separator already wrote — no model, no resampling.

use anyhow::{Context, Result};
use std::path::Path;

/// One input to a mix: a stem WAV and its linear gain (0 = silent, 1 = as split).
pub struct MixInput<'a> {
    pub path: &'a Path,
    pub gain: f32,
}

/// Sum the inputs sample-by-sample at their gains and write a 16-bit WAV to
/// `dest` in the inputs' own format. Every input must share one sample rate
/// and channel count — stems from a single split always do. Inputs at gain 0
/// are skipped without being read. The result is as long as the longest input
/// and clamped to [-1, 1], so pushing every fader past unity clips rather
/// than wraps.
///
/// Returns `(sample_rate, channels)` so the caller can record the file.
pub fn mix_to_wav(inputs: &[MixInput<'_>], dest: &Path) -> Result<(u32, u16)> {
    let audible: Vec<&MixInput<'_>> = inputs.iter().filter(|i| i.gain > 0.0).collect();
    anyhow::ensure!(!audible.is_empty(), "every stem is silent — nothing to mix");

    let mut acc: Vec<f32> = Vec::new();
    let mut spec: Option<(u32, u16)> = None;
    for input in audible {
        let mut reader = hound::WavReader::open(input.path)
            .with_context(|| format!("could not open {}", input.path.display()))?;
        let s = reader.spec();
        match spec {
            None => spec = Some((s.sample_rate, s.channels)),
            Some((sr, ch)) => anyhow::ensure!(
                sr == s.sample_rate && ch == s.channels,
                "{} is {} Hz / {} ch but the mix is {} Hz / {} ch",
                input.path.display(), s.sample_rate, s.channels, sr, ch
            ),
        }
        let samples: Vec<f32> = match s.sample_format {
            hound::SampleFormat::Int => reader
                .samples::<i16>()
                .map(|v| v.map(|x| x as f32 / 32768.0))
                .collect::<std::result::Result<Vec<_>, _>>()?,
            hound::SampleFormat::Float => {
                reader.samples::<f32>().collect::<std::result::Result<Vec<_>, _>>()?
            }
        };
        if samples.len() > acc.len() {
            acc.resize(samples.len(), 0.0);
        }
        for (a, v) in acc.iter_mut().zip(samples) {
            *a += v * input.gain;
        }
    }
    let (sr, ch) = spec.expect("at least one audible input was read");
    anyhow::ensure!(!acc.is_empty(), "the stems contain no audio");
    crate::session::write_wav_i16(dest, &acc, sr, ch)
        .with_context(|| format!("failed writing the mix to {}", dest.display()))?;
    Ok((sr, ch))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, samples: &[f32], sr: u32) -> std::path::PathBuf {
        let p = dir.join(name);
        crate::session::write_wav_i16(&p, samples, sr, 2).unwrap();
        p
    }

    fn read(p: &Path) -> Vec<f32> {
        hound::WavReader::open(p).unwrap()
            .samples::<i16>().map(|s| s.unwrap() as f32 / 32768.0).collect()
    }

    #[test]
    fn mix_sums_inputs_at_their_gains() {
        let dir = tempfile::tempdir().unwrap();
        let a = write(dir.path(), "a.wav", &[0.5, 0.5, 0.5, 0.5], 48_000);
        let b = write(dir.path(), "b.wav", &[0.2, 0.2, 0.2, 0.2], 48_000);
        let out = dir.path().join("mix.wav");
        let (sr, ch) = mix_to_wav(
            &[MixInput { path: &a, gain: 1.0 }, MixInput { path: &b, gain: 0.5 }],
            &out,
        ).unwrap();
        assert_eq!((sr, ch), (48_000, 2));
        for v in read(&out) {
            assert!((v - 0.6).abs() < 0.001, "expected 0.5*1 + 0.2*0.5 = 0.6, got {v}");
        }
    }

    #[test]
    fn silent_inputs_are_skipped_and_all_silent_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let a = write(dir.path(), "a.wav", &[0.5, 0.5], 44_100);
        let missing = dir.path().join("never-read.wav"); // gain 0 must not open it
        let out = dir.path().join("mix.wav");
        mix_to_wav(
            &[MixInput { path: &a, gain: 1.0 }, MixInput { path: &missing, gain: 0.0 }],
            &out,
        ).unwrap();
        assert!((read(&out)[0] - 0.5).abs() < 0.001);

        let err = mix_to_wav(&[MixInput { path: &a, gain: 0.0 }], &out).unwrap_err();
        assert!(err.to_string().contains("silent"), "{err}");
    }

    #[test]
    fn mix_is_as_long_as_the_longest_input_and_clamps() {
        let dir = tempfile::tempdir().unwrap();
        let a = write(dir.path(), "a.wav", &[0.9, 0.9, 0.9, 0.9, 0.9, 0.9], 48_000);
        let b = write(dir.path(), "b.wav", &[0.9, 0.9], 48_000);
        let out = dir.path().join("mix.wav");
        mix_to_wav(&[MixInput { path: &a, gain: 1.0 }, MixInput { path: &b, gain: 1.0 }], &out).unwrap();
        let got = read(&out);
        assert_eq!(got.len(), 6, "length follows the longest stem");
        assert!(got[0] > 0.99, "1.8 must clamp to full scale, got {}", got[0]);
        assert!((got[2] - 0.9).abs() < 0.001, "past b's end only a contributes");
    }

    #[test]
    fn mismatched_formats_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let a = write(dir.path(), "a.wav", &[0.1, 0.1], 48_000);
        let b = write(dir.path(), "b.wav", &[0.1, 0.1], 44_100);
        let out = dir.path().join("mix.wav");
        let err = mix_to_wav(&[MixInput { path: &a, gain: 1.0 }, MixInput { path: &b, gain: 1.0 }], &out).unwrap_err();
        assert!(err.to_string().contains("Hz"), "{err}");
        assert!(!out.exists(), "nothing is written on a refused mix");
    }
}
