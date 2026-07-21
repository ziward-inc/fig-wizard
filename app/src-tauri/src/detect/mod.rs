//! ONNX inference for PP-DocLayoutV3, porting `phase0-spike/detect.py`'s
//! validated preprocessing/postprocessing faithfully rather than
//! re-deriving it.
//!
//! Model IO (verified via onnxruntime introspection in Phase 0):
//!   inputs:  im_shape (N,2) f32, image (N,3,800,800) f32, scale_factor (N,2) f32
//!   outputs: dets (N*maxdet, 7) f32 = [label_idx, score, x0, y0, x1, y1, read_order]
//!            n_valid (N,) i32 = number of valid detections (NMS already baked in)
//!            (a third, unused auxiliary mask output is ignored here)
//!
//! This is a Paddle Detection "DETR"-style export: NMS and box decoding are
//! already baked into the graph, so we only apply a score threshold - no
//! separate NMS pass.

use anyhow::{anyhow, Result};
use image::RgbImage;
use ndarray::Array4;
use ort::session::Session;
use ort::value::Tensor;
use std::path::Path;

use crate::pipeline::types::RawDetection;

pub const TARGET_SIZE: (u32, u32) = (800, 800); // (H, W)
pub const DEFAULT_SCORE_THRESH: f32 = 0.4;

pub struct DocLayoutModel {
    session: Session,
    labels: Vec<String>,
}

impl DocLayoutModel {
    /// Loads the model, registering the CoreML execution provider first
    /// (when built with the `coreml` Cargo feature) with CPU as the
    /// fallback for anything CoreML can't or won't run.
    ///
    /// `ort`'s execution-provider registration is fail-open by default
    /// (`ExecutionProviderDispatch::fail_silently`, which is what
    /// `.build()` produces): if CoreML EP registration itself fails (e.g.
    /// unsupported OS version), `ort` logs a warning and falls back to CPU
    /// for the whole session. And even when CoreML registers successfully,
    /// ONNX Runtime's EP mechanism partitions the graph per-node - any node
    /// CoreML can't take runs on the CPU EP that's always implicitly
    /// available. So no explicit `error_behavior`/fallback plumbing is
    /// needed here: the fallback is automatic on both a "did the EP even
    /// register" and a "does this particular op run on it" level.
    pub fn load(model_path: &Path, labels: Vec<String>) -> Result<Self> {
        #[allow(unused_mut)]
        let mut builder = Session::builder()?;

        #[cfg(feature = "coreml")]
        {
            // `with_execution_providers` returns `Result<SessionBuilder,
            // ort::Error<SessionBuilder>>` (the builder is threaded through
            // the error type so it can be recovered on failure), which
            // means the error type embeds a `SessionBuilder` and isn't
            // `Send + Sync` - so it can't go through `anyhow`'s blanket `?`
            // conversion. Format it to a string instead of propagating the
            // raw error type.
            builder = builder
                .with_execution_providers([ort::ep::CoreML::default()
                    .with_compute_units(ort::ep::coreml::ComputeUnits::All)
                    .build()])
                .map_err(|e| anyhow!("failed to register CoreML execution provider: {e}"))?;
        }

        let session = builder.commit_from_file(model_path)?;
        Ok(Self { session, labels })
    }

    /// Runs detection on a single already-resized-to-800x800 RGB image.
    /// `scale_h`/`scale_w` are `target/orig` as computed by
    /// `pdf::render::resize_for_model` and are fed to the model exactly as
    /// `detect.py` does (the model uses these to internally rescale its
    /// output boxes back into the *original*, pre-resize pixel space - the
    /// boxes this function returns are in that original space, i.e. the
    /// pixel space of whatever page raster was fed into
    /// `resize_for_model`).
    pub fn run(
        &mut self,
        resized_rgb: &RgbImage,
        scale_h: f32,
        scale_w: f32,
        score_thresh: f32,
    ) -> Result<Vec<RawDetection>> {
        let (w, h) = (resized_rgb.width(), resized_rgb.height());
        if (w, h) != (TARGET_SIZE.1, TARGET_SIZE.0) {
            return Err(anyhow!(
                "expected {}x{} input, got {}x{}",
                TARGET_SIZE.1,
                TARGET_SIZE.0,
                w,
                h
            ));
        }

        // RGB, /255, CHW - see detect.py::preprocess docstring for why this
        // exact recipe (no ImageNet mean/std) is the validated one.
        let mut chw = Array4::<f32>::zeros((1, 3, h as usize, w as usize));
        for y in 0..h {
            for x in 0..w {
                let px = resized_rgb.get_pixel(x, y);
                for c in 0..3 {
                    chw[[0, c, y as usize, x as usize]] = px[c] as f32 / 255.0;
                }
            }
        }

        let im_shape = ndarray::Array2::<f32>::from_shape_vec(
            (1, 2),
            vec![TARGET_SIZE.0 as f32, TARGET_SIZE.1 as f32],
        )?;
        let scale_factor =
            ndarray::Array2::<f32>::from_shape_vec((1, 2), vec![scale_h, scale_w])?;

        let im_shape_value = Tensor::from_array(im_shape)?;
        let image_value = Tensor::from_array(chw)?;
        let scale_factor_value = Tensor::from_array(scale_factor)?;

        let input_names: Vec<String> = self
            .session
            .inputs()
            .iter()
            .map(|i| i.name().to_string())
            .collect();
        if input_names.len() != 3 {
            return Err(anyhow!(
                "expected 3 model inputs (im_shape, image, scale_factor), found {}",
                input_names.len()
            ));
        }

        let output_count = self.session.outputs().len();

        let outputs = self.session.run(ort::inputs![
            input_names[0].as_str() => im_shape_value,
            input_names[1].as_str() => image_value,
            input_names[2].as_str() => scale_factor_value,
        ])?;

        let dets_tensor = outputs[0].try_extract_array::<f32>()?;
        let n_valid: i64 = if output_count > 1 {
            let n_tensor = outputs[1].try_extract_array::<i32>()?;
            *n_tensor.iter().next().unwrap_or(&0) as i64
        } else {
            dets_tensor.shape()[0] as i64
        };

        let dets = dets_tensor.into_dimensionality::<ndarray::Ix2>()?;
        let n_valid = (n_valid.max(0) as usize).min(dets.shape()[0]);

        let mut results = Vec::new();
        for row_idx in 0..n_valid {
            let row = dets.row(row_idx);
            let label_idx = row[0] as i64;
            let score = row[1];
            if score < score_thresh {
                continue;
            }
            let (x0, y0, x1, y1) = (row[2], row[3], row[4], row[5]);

            let label = if label_idx >= 0 && (label_idx as usize) < self.labels.len() {
                self.labels[label_idx as usize].clone()
            } else {
                format!("cls_{label_idx}")
            };

            results.push(RawDetection {
                label_idx: label_idx.max(0) as usize,
                label,
                score,
                px_x0: x0,
                px_y0: y0,
                px_x1: x1,
                px_y1: y1,
            });
        }

        Ok(results)
    }
}
