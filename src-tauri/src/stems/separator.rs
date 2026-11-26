//! ONNX Runtime inference for one fixed-size chunk.
//!
//! `ort` was chosen over pure-Rust `tract` by measurement, not preference:
//! `tract` cannot parse htdemucs (2-input `Pad` at node #1104). See the
//! stem-isolation spec §1 RESOLVED. Expect ~4.9 GB peak RSS per inference —
//! callers MUST serialise these calls.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use super::chunk::CHUNK;

/// Output order of the model's `stems` tensor. Mislabelling these silently swaps
/// the contents of every stem file, so it is asserted in tests.
pub const STEM_NAMES: [&str; 4] = ["drums", "bass", "other", "vocals"];

/// One fixed-size chunk in, four stems out. Behind a trait so the pipeline can be
/// exercised with a stub in tests that must not load a 166 MB model.
pub trait Separator {
    /// `mix`: 2 planar channels, each exactly `CHUNK` samples, values in [-1, 1].
    /// Returns 4 stems, each 2 channels laid out planar as `[L…, R…]`.
    fn separate_chunk(&mut self, mix: &[Vec<f32>]) -> Result<[Vec<f32>; 4]>;
}

pub struct OrtSeparator {
    session: ort::session::Session,
}

impl OrtSeparator {
    pub fn new(model_path: &Path) -> Result<OrtSeparator> {
        if !model_path.exists() {
            anyhow::bail!("stem separation model not found at {}", model_path.display());
        }
        // ort's error type is generic over the builder stage and does not satisfy
        // anyhow::Context's bounds, so map explicitly rather than using `.context`.
        let session = ort::session::Session::builder()
            .map_err(|e| anyhow::anyhow!("failed to create an ONNX Runtime session builder: {e}"))?
            .with_optimization_level(ort::session::builder::GraphOptimizationLevel::Level3)
            .map_err(|e| anyhow::anyhow!("failed to set the graph optimization level: {e}"))?
            .commit_from_file(model_path)
            .map_err(|e| {
                anyhow::anyhow!("failed to load the model at {}: {e}", model_path.display())
            })?;
        Ok(OrtSeparator { session })
    }
}

impl Separator for OrtSeparator {
    fn separate_chunk(&mut self, mix: &[Vec<f32>]) -> Result<[Vec<f32>; 4]> {
        anyhow::ensure!(mix.len() == 2, "model expects stereo, got {} channel(s)", mix.len());
        anyhow::ensure!(
            mix[0].len() == CHUNK && mix[1].len() == CHUNK,
            "model expects exactly {CHUNK} samples per channel"
        );

        let mut flat = Vec::with_capacity(2 * CHUNK);
        flat.extend_from_slice(&mix[0]);
        flat.extend_from_slice(&mix[1]);
        let arr = ndarray::Array3::from_shape_vec((1, 2, CHUNK), flat)
            .context("failed to shape the input tensor")?;

        let outputs = self
            .session
            .run(ort::inputs!["mix" => ort::value::TensorRef::from_array_view(&arr)?])
            .context("ONNX inference failed")?;
        let (shape, data) = outputs["stems"]
            .try_extract_tensor::<f32>()
            .context("failed to read the stems tensor")?;
        anyhow::ensure!(
            shape.len() == 4 && shape[1] == 4 && shape[2] == 2 && shape[3] as usize == CHUNK,
            "unexpected output shape {shape:?}, expected (1, 4, 2, {CHUNK})"
        );

        let per_stem = 2 * CHUNK;
        let mut out: [Vec<f32>; 4] = Default::default();
        for s in 0..4 {
            out[s] = data[s * per_stem..(s + 1) * per_stem].to_vec();
        }
        Ok(out)
    }
}

/// Where the bundled model lives at runtime. In a packaged build Tauri unpacks
/// `bundle.resources` next to the binary; in `tauri dev` it resolves inside
/// `src-tauri/`.
pub fn resolve_model_path(app: &tauri::AppHandle) -> Result<PathBuf> {
    use tauri::Manager;
    app.path()
        .resolve(
            "resources/models/htdemucs_fp16weights.onnx",
            tauri::path::BaseDirectory::Resource,
        )
        .context("could not resolve the bundled model path")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stem_names_are_the_models_output_order() {
        // The model emits (1, 4, 2, N) with this exact ordering. Getting it wrong
        // silently mislabels every stem, which no automated check downstream would
        // catch — the drums file would simply contain bass.
        assert_eq!(STEM_NAMES, ["drums", "bass", "other", "vocals"]);
    }

    #[test]
    fn missing_model_is_a_clear_error_not_a_panic() {
        // `unwrap_err` would require Debug on OrtSeparator, which wraps a non-Debug
        // ort Session — match instead.
        let err = match OrtSeparator::new(std::path::Path::new("does/not/exist.onnx")) {
            Ok(_) => panic!("expected an error for a missing model file"),
            Err(e) => e,
        };
        let msg = err.to_string().to_lowercase();
        assert!(msg.contains("model"), "error should mention the model: {err}");
    }

    /// Requires the 166 MB model on disk. Run with:
    ///   cargo test --manifest-path src-tauri/Cargo.toml -- --ignored
    #[test]
    #[ignore]
    fn ort_separates_one_chunk_into_four_finite_stems() {
        let path = std::path::Path::new("resources/models/htdemucs_fp16weights.onnx");
        if !path.exists() {
            panic!("run `npm run fetch-model` first");
        }
        let mut sep = OrtSeparator::new(path).unwrap();

        let n = crate::stems::chunk::CHUNK;
        let mix: Vec<Vec<f32>> = (0..2)
            .map(|c| {
                (0..n)
                    .map(|i| {
                        ((i as f32 / 44_100.0) * (220.0 + 110.0 * c as f32) * std::f32::consts::TAU)
                            .sin()
                            * 0.4
                    })
                    .collect()
            })
            .collect();

        let stems = sep.separate_chunk(&mix).unwrap();
        for (i, s) in stems.iter().enumerate() {
            assert_eq!(s.len(), 2 * n, "stem {} ({}) must be 2 channels x CHUNK", i, STEM_NAMES[i]);
            assert!(s.iter().all(|v| v.is_finite()), "stem {} has NaN/inf", STEM_NAMES[i]);
        }
        let energy: f32 = stems.iter().flat_map(|s| s.iter()).map(|v| v.abs()).sum();
        assert!(energy > 0.0, "all four stems are silent — the model ran but produced nothing");
    }
}
