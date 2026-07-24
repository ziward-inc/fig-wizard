//! Orchestrates the whole render -> detect -> associate -> crop -> export
//! pipeline for one PDF, independent of Tauri so it can be exercised from
//! plain `cargo test` as well as from `commands.rs`.

use anyhow::{Context, Result};
use pdfium_render::prelude::Pdfium;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::detect::{DocLayoutModel, TARGET_SIZE};
use crate::pdf::render::{pixel_box_to_pdf_points, render_page_for_detection, resize_for_model, ClipRenderBudget};
use crate::pipeline::associate::associate_page;
use crate::pipeline::export::{export_object, manifest_entry, write_manifest};
use crate::pipeline::types::{
    Manifest, ManifestEntry, OutputFormat, PageDetection, VerificationInfo, VerifyBackend,
};
use crate::verify;

/// DPI used for the full-page detection-pass render (matches the 200 DPI
/// used to validate the model during the initial spike).
pub const DETECTION_DPI: f32 = 200.0;

/// Safety margin added to every side of a bbox returned by the detection
/// model. The ratio is relative to the bbox's width or height respectively.
const DETECTION_BBOX_MARGIN_RATIO: f32 = 0.05;

#[derive(Debug, Clone)]
pub enum PipelineEvent {
    PageDetected {
        page_index: u32,
        page_count: u32,
        counts_by_kind: HashMap<String, u32>,
    },
    ObjectExported {
        id: String,
        kind: String,
        page_index: u32,
    },
    ExtractionComplete {
        manifest_path: PathBuf,
        object_count: u32,
    },
}

pub struct ProcessPdfParams<'a> {
    pub pdfium: &'a Pdfium,
    pub pdf_path: &'a Path,
    pub output_dir: &'a Path,
    pub model_path: &'a Path,
    pub labels: Vec<String>,
    pub score_thresh: f32,
    pub clip_budget: ClipRenderBudget,
    /// The single image format every exported crop in this run is encoded
    /// as (see `pipeline::types::OutputFormat` - this app used to always
    /// export both WebP and AVIF; now the caller picks one).
    pub output_format: OutputFormat,
    /// Off by default: when set to `Codex` or `Claude`, each detected
    /// object's crop is checked (and, if needed, corrected and re-checked)
    /// via that CLI before export - see `crate::verify`. Requires network
    /// access and meaningfully increases extraction time, hence opt-in.
    pub verify_backend: VerifyBackend,
}

/// Runs the full pipeline for one PDF. `on_event` is called for progress
/// reporting (page-detected / object-exported / extraction-complete);
/// `cancel` is polled between pages and objects so a long-running
/// extraction can be aborted from the UI.
pub fn process_pdf(
    params: ProcessPdfParams,
    cancel: &AtomicBool,
    mut on_event: impl FnMut(PipelineEvent),
) -> Result<Manifest> {
    let pdf_stem = params
        .pdf_path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "document".to_string());

    let doc_out_dir = params.output_dir.join(&pdf_stem);
    std::fs::create_dir_all(&doc_out_dir)
        .with_context(|| format!("creating output dir {doc_out_dir:?}"))?;

    let doc = params
        .pdfium
        .load_pdf_from_file(params.pdf_path, None)
        .with_context(|| format!("loading pdf {:?}", params.pdf_path))?;

    let page_count = doc.pages().len() as u32;

    let mut model = DocLayoutModel::load(params.model_path, params.labels)
        .context("loading ONNX detection model")?;

    let mut entries: Vec<ManifestEntry> = Vec::new();

    // Global (whole-document) per-kind counter: object filenames live flat
    // in `doc_out_dir` (no per-page subfolder), so `figure-01`, `figure-02`,
    // ... must stay unique across every page rather than resetting each page.
    let mut seq_by_kind: HashMap<&'static str, u32> = HashMap::new();

    for page_index in 0..page_count {
        if cancel.load(Ordering::Relaxed) {
            break;
        }

        let page = doc
            .pages()
            .get(page_index as i32)
            .with_context(|| format!("getting page {page_index}"))?;

        let (page_img, geometry) = render_page_for_detection(&page, DETECTION_DPI)
            .with_context(|| format!("rendering page {page_index} for detection"))?;
        let (resized, scale_h, scale_w) = resize_for_model(&page_img, TARGET_SIZE.1, TARGET_SIZE.0);

        let raw_dets = model
            .run(&resized, scale_h, scale_w, params.score_thresh)
            .with_context(|| format!("running detection on page {page_index}"))?;

        let page_dets: Vec<PageDetection> = raw_dets
            .iter()
            .map(|d| {
                let bbox_pt = pixel_box_to_pdf_points(
                    d.px_x0,
                    d.px_y0,
                    d.px_x1,
                    d.px_y1,
                    DETECTION_DPI,
                    geometry.height_pt,
                )
                .expanded_by_ratio(
                    DETECTION_BBOX_MARGIN_RATIO,
                    geometry.width_pt,
                    geometry.height_pt,
                );

                PageDetection {
                    label: d.label.clone(),
                    score: d.score,
                    bbox_pt,
                }
            })
            .collect();

        let mut objects = associate_page(page_index, &page_dets, geometry.width_pt);

        let mut counts_by_kind: HashMap<String, u32> = HashMap::new();
        for obj in &objects {
            *counts_by_kind.entry(obj.kind.as_str().to_string()).or_insert(0) += 1;
        }
        on_event(PipelineEvent::PageDetected {
            page_index,
            page_count,
            counts_by_kind,
        });

        for obj in &mut objects {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let seq = seq_by_kind.entry(obj.kind.as_str()).or_insert(0);
            *seq += 1;

            let verification = if params.verify_backend != VerifyBackend::Off {
                let verify_dir = std::env::temp_dir().join(format!("pdf-extractor-verify-{}", obj.id));
                let outcome = verify::verify_and_correct_crop(
                    &page,
                    obj.kind.as_str(),
                    obj.bbox_pt,
                    params.clip_budget,
                    verify::MAX_ATTEMPTS,
                    &verify_dir,
                    cancel,
                    params.verify_backend,
                )
                .with_context(|| format!("verifying crop for object {}", obj.id))?;
                let _ = std::fs::remove_dir_all(&verify_dir);

                let (_img, corrected_bbox, verify_outcome) = outcome;
                // The corrected object-only bbox feeds BOTH the no-caption
                // crop and (via `with_caption_bbox()`, re-unioned with the
                // original caption box below) the with-caption crop - one
                // verification pass per object covers both file variants.
                obj.bbox_pt = corrected_bbox;

                Some(VerificationInfo {
                    enabled: true,
                    attempts: verify_outcome.attempts,
                    passed: verify_outcome.passed,
                    last_issue: verify_outcome.last_issue,
                    history: verify_outcome.history,
                })
            } else {
                None
            };

            let files = export_object(
                &page,
                obj,
                &doc_out_dir,
                page_index + 1,
                *seq,
                params.clip_budget,
                params.output_format,
            )
            .with_context(|| format!("exporting object {}", obj.id))?;

            on_event(PipelineEvent::ObjectExported {
                id: obj.id.clone(),
                kind: obj.kind.as_str().to_string(),
                page_index,
            });

            entries.push(manifest_entry(obj, files, verification));
        }
    }

    let manifest = Manifest {
        source_pdf: params.pdf_path.to_string_lossy().to_string(),
        page_count,
        objects: entries,
    };

    let manifest_path = write_manifest(&doc_out_dir, &manifest)?;

    on_event(PipelineEvent::ExtractionComplete {
        manifest_path,
        object_count: manifest.objects.len() as u32,
    });

    Ok(manifest)
}
