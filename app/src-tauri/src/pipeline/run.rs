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
use crate::pipeline::types::{Manifest, ManifestEntry, PageDetection};

/// DPI used for the full-page detection-pass render (matches the 200 DPI
/// used to validate the model in Phase 0 - see phase0-spike/render_pages.py).
pub const DETECTION_DPI: f32 = 200.0;

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
            .map(|d| PageDetection {
                label: d.label.clone(),
                score: d.score,
                bbox_pt: pixel_box_to_pdf_points(
                    d.px_x0,
                    d.px_y0,
                    d.px_x1,
                    d.px_y1,
                    DETECTION_DPI,
                    geometry.height_pt,
                ),
            })
            .collect();

        let objects = associate_page(page_index, &page_dets, geometry.width_pt);

        let mut counts_by_kind: HashMap<String, u32> = HashMap::new();
        for obj in &objects {
            *counts_by_kind.entry(obj.kind.as_str().to_string()).or_insert(0) += 1;
        }
        on_event(PipelineEvent::PageDetected {
            page_index,
            page_count,
            counts_by_kind,
        });

        let page_dir = doc_out_dir.join(format!("page-{:04}", page_index + 1));

        let mut seq_by_kind: HashMap<&'static str, u32> = HashMap::new();
        for obj in &objects {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let seq = seq_by_kind.entry(obj.kind.as_str()).or_insert(0);
            *seq += 1;

            let files = export_object(&page, obj, &page_dir, *seq, params.clip_budget)
                .with_context(|| format!("exporting object {}", obj.id))?;

            on_event(PipelineEvent::ObjectExported {
                id: obj.id.clone(),
                kind: obj.kind.as_str().to_string(),
                page_index,
            });

            entries.push(manifest_entry(obj, files));
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
