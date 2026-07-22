//! Per-object export: render both the object-only and object+caption
//! crops (skipping the re-render when there's no caption), encode each to
//! WebP q85 and AVIF q85, and write a `manifest.json` for the whole PDF.

use anyhow::{anyhow, Context, Result};
use image::RgbImage;
use pdfium_render::prelude::PdfPage;
use std::fs;
use std::path::{Path, PathBuf};

use crate::pdf::render::{render_clip, ClipRenderBudget};
use crate::pipeline::types::{DetectedObject, ExportedFiles, Manifest, ManifestEntry, VerificationInfo};

/// WebP encode at a fixed quality. Uses the `webp` crate (libwebp
/// bindings), NOT `image`'s built-in WebP encoder, which only supports
/// lossless encoding.
fn encode_webp(img: &RgbImage, quality: f32) -> Result<Vec<u8>> {
    let encoder = webp::Encoder::from_rgb(img.as_raw(), img.width(), img.height());
    let mem = encoder.encode(quality);
    Ok(mem.to_vec())
}

/// AVIF encode at a fixed quality via `ravif`. Speed 6-8 keeps encode times
/// reasonable at near-4K resolution.
fn encode_avif(img: &RgbImage, quality: f32) -> Result<Vec<u8>> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let rgb_pixels: Vec<rgb::RGB8> = img
        .pixels()
        .map(|p| rgb::RGB8::new(p[0], p[1], p[2]))
        .collect();
    let img_buf = ravif::Img::new(rgb_pixels.as_slice(), w, h);

    let result = ravif::Encoder::new()
        .with_quality(quality)
        .with_speed(7)
        .encode_rgb(img_buf)
        .map_err(|e| anyhow!("avif encode failed: {e}"))?;

    Ok(result.avif_file)
}

/// Renders and encodes the four variants for one object, writing them
/// under `<page_dir>/<kind>-NN_{with,no}-caption_q85.{webp,avif}`.
pub fn export_object(
    page: &PdfPage,
    obj: &DetectedObject,
    page_dir: &Path,
    seq_in_page: u32,
    budget: ClipRenderBudget,
) -> Result<ExportedFiles> {
    fs::create_dir_all(page_dir)
        .with_context(|| format!("creating page output dir {page_dir:?}"))?;

    let base = format!("{}-{:02}", obj.kind.as_str(), seq_in_page);

    let no_caption_bbox = obj.bbox_pt;
    let with_caption_bbox = obj.with_caption_bbox();
    let has_caption = obj.caption_bbox_pt.is_some();

    let no_caption_img = render_clip(page, no_caption_bbox, budget)
        .with_context(|| format!("rendering object-only clip for {}", obj.id))?;

    // If there's no caption, the with-caption crop is identical to the
    // object-only crop - reuse the same bitmap rather than re-rendering.
    let with_caption_img = if has_caption {
        render_clip(page, with_caption_bbox, budget)
            .with_context(|| format!("rendering with-caption clip for {}", obj.id))?
    } else {
        no_caption_img.clone()
    };

    let no_caption_webp_path = page_dir.join(format!("{base}_no-caption_q85.webp"));
    let with_caption_webp_path = page_dir.join(format!("{base}_with-caption_q85.webp"));
    let no_caption_avif_path = page_dir.join(format!("{base}_no-caption_q85.avif"));
    let with_caption_avif_path = page_dir.join(format!("{base}_with-caption_q85.avif"));

    fs::write(&no_caption_webp_path, encode_webp(&no_caption_img, 85.0)?)?;
    fs::write(&with_caption_webp_path, encode_webp(&with_caption_img, 85.0)?)?;
    fs::write(&no_caption_avif_path, encode_avif(&no_caption_img, 85.0)?)?;
    fs::write(&with_caption_avif_path, encode_avif(&with_caption_img, 85.0)?)?;

    Ok(ExportedFiles {
        with_caption_webp: with_caption_webp_path.to_string_lossy().to_string(),
        no_caption_webp: no_caption_webp_path.to_string_lossy().to_string(),
        with_caption_avif: with_caption_avif_path.to_string_lossy().to_string(),
        no_caption_avif: no_caption_avif_path.to_string_lossy().to_string(),
    })
}

/// Builds a `ManifestEntry` from a `DetectedObject` and its exported file
/// paths. `verification` is `None` whenever the (off-by-default) Codex
/// crop-verification pass wasn't enabled for this run.
pub fn manifest_entry(
    obj: &DetectedObject,
    files: ExportedFiles,
    verification: Option<VerificationInfo>,
) -> ManifestEntry {
    let with_caption = obj.with_caption_bbox();
    ManifestEntry {
        id: obj.id.clone(),
        kind: obj.kind.as_str().to_string(),
        raw_label: obj.raw_label.clone(),
        page_index: obj.page_index,
        score: obj.score,
        bbox_pt: [obj.bbox_pt.x0, obj.bbox_pt.y0, obj.bbox_pt.x1, obj.bbox_pt.y1],
        with_caption_bbox_pt: [with_caption.x0, with_caption.y0, with_caption.x1, with_caption.y1],
        has_caption: obj.caption_bbox_pt.is_some(),
        files,
        verification,
    }
}

/// Writes `manifest.json` for a whole PDF into `output_dir/<pdf-stem>/`.
pub fn write_manifest(output_dir: &Path, manifest: &Manifest) -> Result<PathBuf> {
    let path = output_dir.join("manifest.json");
    let json = serde_json::to_string_pretty(manifest)?;
    fs::write(&path, json)?;
    Ok(path)
}
