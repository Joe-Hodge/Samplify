//! Neural stem separation (drums / bass / other / vocals) via ONNX Runtime.
//!
//! Layered so the maths is testable without the 166 MB model present:
//! `chunk` is pure arithmetic, `resample` needs only rubato, and `separator`
//! is the only part that touches `ort`.
pub mod chunk;
pub mod resample;
pub mod job;
pub mod separator;
pub mod mix;

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// The rate htdemucs was trained at and requires.
pub const MODEL_HZ: u32 = 44_100;

/// Read a capture, separate it, and write four stem WAVs into `out_dir`.
///
/// The source is opened read-only and never modified. Returns the four written
/// paths in `separator::STEM_NAMES` order. `on_progress(done, total)` fires once
/// per chunk so a UI can show real progress on a job that takes seconds; returning
/// false from it aborts the job before the next chunk.
pub fn separate_to_files(
    sep: &mut dyn separator::Separator,
    src: &Path,
    out_dir: &Path,
    artist: &str,
    title: &str,
    on_progress: &mut dyn FnMut(usize, usize) -> bool,
) -> Result<Vec<PathBuf>> {
    // 1. Read the capture as planar f32 stereo.
    let mut reader = hound::WavReader::open(src)
        .with_context(|| format!("could not open {}", src.display()))?;
    let spec = reader.spec();
    let src_hz = spec.sample_rate;
    let src_ch = spec.channels.max(1) as usize;
    let raw: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => reader
            .samples::<i16>()
            .map(|s| s.map(|v| v as f32 / 32768.0))
            .collect::<std::result::Result<Vec<_>, _>>()?,
        hound::SampleFormat::Float => {
            reader.samples::<f32>().collect::<std::result::Result<Vec<_>, _>>()?
        }
    };
    let frames = raw.len() / src_ch;
    anyhow::ensure!(frames > 0, "capture contains no audio");

    // Mono is duplicated: the model requires stereo.
    let mut planar: Vec<Vec<f32>> = vec![Vec::with_capacity(frames); 2];
    for f in 0..frames {
        let l = raw[f * src_ch];
        let r = if src_ch > 1 { raw[f * src_ch + 1] } else { l };
        planar[0].push(l);
        planar[1].push(r);
    }

    // 2. To the model's rate.
    let planar = resample::resample(&planar, src_hz, MODEL_HZ)?;
    let len = planar[0].len();

    // 3. Chunk, infer, overlap-add.
    let starts = chunk::chunk_starts(len);
    let total = starts.len();
    let last = total - 1;
    let mut acc: Vec<Vec<Vec<f32>>> = (0..4)
        .map(|_| vec![vec![0.0f32; len], vec![0.0f32; len]])
        .collect();

    for (i, &s) in starts.iter().enumerate() {
        let mut win: Vec<Vec<f32>> = vec![vec![0.0f32; chunk::CHUNK], vec![0.0f32; chunk::CHUNK]];
        for c in 0..2 {
            for j in 0..chunk::CHUNK {
                if s + j < len {
                    win[c][j] = planar[c][s + j];
                }
            }
        }
        let out = sep.separate_chunk(&win)?;
        for st in 0..4 {
            for c in 0..2 {
                let slice = &out[st][c * chunk::CHUNK..(c + 1) * chunk::CHUNK];
                chunk::overlap_add(&mut acc[st][c], slice, s, i == 0, i == last);
            }
        }
        if !on_progress(i + 1, total) {
            anyhow::bail!("separation cancelled");
        }
    }

    // 4. Back to the source rate, interleave, write.
    std::fs::create_dir_all(out_dir)?;
    let mut written = Vec::with_capacity(4);
    for (st, name) in separator::STEM_NAMES.iter().enumerate() {
        let mut back = resample::resample(&acc[st], MODEL_HZ, src_hz)?;
        // The sinc filter cannot produce its final ~2*sinc_len samples, so a
        // roundtrip returns slightly short. Pad back to the source frame count so
        // every stem stays sample-locked to the mix it came from — otherwise they
        // drift against the original when layered in the Playground.
        for c in back.iter_mut() {
            c.resize(frames, 0.0);
        }
        let mut interleaved = Vec::with_capacity(frames * 2);
        for f in 0..frames {
            interleaved.push(back[0][f].clamp(-1.0, 1.0));
            interleaved.push(back[1][f].clamp(-1.0, 1.0));
        }
        let fname = crate::session::sanitize_filename(artist, title, name);
        let dest = out_dir.join(fname);
        crate::session::write_wav_i16(&dest, &interleaved, src_hz, 2)
            .with_context(|| format!("failed writing the {name} stem"))?;
        written.push(dest);
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stems::separator::Separator;

    /// Returns the input untouched as `drums`, silence elsewhere. Lets the whole
    /// pipeline be tested end-to-end with no 166 MB model present.
    struct PassthroughSeparator;
    impl Separator for PassthroughSeparator {
        fn separate_chunk(&mut self, mix: &[Vec<f32>]) -> Result<[Vec<f32>; 4]> {
            let mut drums = Vec::with_capacity(2 * chunk::CHUNK);
            drums.extend_from_slice(&mix[0]);
            drums.extend_from_slice(&mix[1]);
            let silent = || vec![0.0f32; 2 * chunk::CHUNK];
            Ok([drums, silent(), silent(), silent()])
        }
    }

    #[test]
    fn separate_to_files_writes_four_stems_locked_to_the_source_length() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("mix.wav");

        // ~2.2 chunks at 44.1 kHz, so overlap-add is genuinely exercised.
        let n = (chunk::CHUNK as f64 * 2.2) as usize;
        let mut interleaved = Vec::with_capacity(n * 2);
        for i in 0..n {
            let v = ((i as f32 / 44_100.0) * 300.0 * std::f32::consts::TAU).sin() * 0.5;
            interleaved.push(v);
            interleaved.push(v * 0.8);
        }
        crate::session::write_wav_i16(&src, &interleaved, 44_100, 2).unwrap();

        let mut calls = 0usize;
        let paths = {
            let mut on_progress = |_done: usize, _total: usize| {
                calls += 1;
                true
            };
            separate_to_files(
                &mut PassthroughSeparator,
                &src,
                dir.path(),
                "Artist",
                "Title",
                &mut on_progress,
            )
            .unwrap()
        };

        assert_eq!(paths.len(), 4);
        for (p, name) in paths.iter().zip(separator::STEM_NAMES) {
            assert!(p.exists(), "{name} stem missing");
            assert!(
                p.to_string_lossy().contains(name),
                "{} should name the stem",
                p.display()
            );
        }
        assert!(calls >= 3, "progress should fire per chunk, got {calls}");

        // Every stem is exactly as long as the source, so they layer without drift.
        let a = hound::WavReader::open(&src).unwrap().duration();
        for (p, name) in paths.iter().zip(separator::STEM_NAMES) {
            let b = hound::WavReader::open(p).unwrap().duration();
            assert_eq!(a, b, "{name} stem is {b} frames, source is {a}");
        }
    }

    #[test]
    fn separate_to_files_rejects_a_missing_source() {
        let dir = tempfile::tempdir().unwrap();
        let mut noop = |_: usize, _: usize| true;
        let err = separate_to_files(
            &mut PassthroughSeparator,
            &dir.path().join("nope.wav"),
            dir.path(),
            "a",
            "b",
            &mut noop,
        )
        .unwrap_err();
        assert!(!err.to_string().is_empty());
    }
}

#[cfg(test)]
mod audition {
    use super::*;
    /// Bakes four real stems from a real capture so the owner can LISTEN.
    ///   SAMPLIFY_CLIP="<path>" cargo test --manifest-path src-tauri/Cargo.toml --lib bake_real -- --ignored --nocapture
    #[test]
    #[ignore]
    fn bake_real_stems_for_audition() {
        let src = std::path::PathBuf::from(std::env::var("SAMPLIFY_CLIP").expect("set SAMPLIFY_CLIP"));
        let model = std::path::Path::new("resources/models/htdemucs_fp16weights.onnx");
        let mut sep = separator::OrtSeparator::new(model).unwrap();
        let out = std::path::Path::new("target/stem-audition");
        let t0 = std::time::Instant::now();
        let mut prog = |d: usize, t: usize| { println!("  chunk {d}/{t}"); true };
        let paths = separate_to_files(&mut sep, &src, out, "audition", "clip", &mut prog).unwrap();
        println!("TOOK {:?}", t0.elapsed());
        for p in &paths { println!("OUT {}", p.display()); }
    }
}
