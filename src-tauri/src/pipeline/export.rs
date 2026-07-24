//! Per-object export: render both the object-only and object+caption
//! crops (skipping the re-render when there's no caption), encode each in
//! the user-selected `OutputFormat` at quality 85 (or losslessly for PNG),
//! and write a `manifest.json` for the whole PDF.

use anyhow::{anyhow, Context, Result};
use gamut_core::{Dimensions, EncodeImage, ImageRef, Rgb8};
use gamut_jxl::{Distance, JxlEncoder};
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageEncoder, RgbImage};
use pdfium_render::prelude::PdfPage;
use std::fs;
use std::path::{Path, PathBuf};

use crate::pdf::render::{render_clip, ClipRenderBudget};
use crate::pipeline::types::{
    DetectedObject, ExportedFiles, Manifest, ManifestEntry, OutputFormat, VerificationInfo,
};
/// JPEG-style quality (0-100) applied to every lossy format. PNG is
/// lossless and ignores this.
const QUALITY: u8 = 85;

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

/// PNG encode (lossless) via `image`'s built-in encoder - no quality knob.
fn encode_png(img: &RgbImage) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    PngEncoder::new(&mut buf)
        .write_image(img.as_raw(), img.width(), img.height(), ExtendedColorType::Rgb8)
        .map_err(|e| anyhow!("png encode failed: {e}"))?;
    Ok(buf)
}

/// JPEG encode at a fixed quality via `image`'s built-in encoder.
fn encode_jpeg(img: &RgbImage, quality: u8) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    JpegEncoder::new_with_quality(&mut buf, quality)
        .write_image(img.as_raw(), img.width(), img.height(), ExtendedColorType::Rgb8)
        .map_err(|e| anyhow!("jpeg encode failed: {e}"))?;
    Ok(buf)
}

/// JPEG XL encode via the reference libjxl encoder, statically linked into
/// the app. `cjxl -q QUALITY` uses `JxlEncoderDistanceFromQuality`; this
/// reproduces that public mapping before passing the resulting Butteraugli
/// distance to the linked encoder.
fn encode_jpegxl(img: &RgbImage, quality: u8) -> Result<Vec<u8>> {
    let distance = jpegxl_distance_from_quality(quality);
    let distance = Distance::new(distance)
        .map_err(|e| anyhow!("invalid JPEG XL quality {quality}: {e}"))?;
    let dimensions = Dimensions { width: img.width(), height: img.height() };
    let image = ImageRef::<Rgb8>::new(img.as_raw(), dimensions)
        .map_err(|e| anyhow!("creating JPEG XL RGB image view: {e}"))?;

    JxlEncoder::lossy(distance)
        .encode_to_vec(image)
        .map_err(|e| anyhow!("jpeg xl encode failed: {e}"))
}

fn jpegxl_distance_from_quality(quality: u8) -> f32 {
    let quality = f32::from(quality);
    if quality >= 100.0 {
        0.0
    } else if quality >= 30.0 {
        0.1 + (100.0 - quality) * 0.09
    } else {
        53.0 / 3000.0 * quality * quality - 23.0 / 20.0 * quality + 25.0
    }
}

/// Encodes `img` in the requested `format` at the app-wide fixed quality
/// (`QUALITY` for lossy formats; ignored for PNG).
fn encode(img: &RgbImage, format: OutputFormat) -> Result<Vec<u8>> {
    match format {
        OutputFormat::Webp => encode_webp(img, QUALITY as f32),
        OutputFormat::Avif => encode_avif(img, QUALITY as f32),
        OutputFormat::Png => encode_png(img),
        OutputFormat::Jpeg => encode_jpeg(img, QUALITY),
        OutputFormat::JpegXl => encode_jpegxl(img, QUALITY),
    }
}

/// Renders and encodes the with/without-caption variants for one object in
/// the caller-selected `output_format`, writing them flat into `out_dir`
/// (no per-page subfolder) as
/// `p<PP>_<kind>-NN_{with,no}-caption[_q85].<ext>` (the `_q85` quality
/// suffix is omitted for PNG, since it's lossless). `page_number` is
/// 1-indexed; `seq` is a whole-document counter per `kind` (not reset per
/// page), since filenames for every page now share one flat directory.
pub fn export_object(
    page: &PdfPage,
    obj: &DetectedObject,
    out_dir: &Path,
    page_number: u32,
    seq: u32,
    budget: ClipRenderBudget,
    output_format: OutputFormat,
) -> Result<ExportedFiles> {
    fs::create_dir_all(out_dir)
        .with_context(|| format!("creating output dir {out_dir:?}"))?;

    let base = format!("p{:02}_{}-{:02}", page_number, obj.kind.as_str(), seq);

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

    let ext = output_format.extension();
    let quality_suffix = if output_format.is_lossless() { "" } else { "_q85" };
    let no_caption_path = out_dir.join(format!("{base}_no-caption{quality_suffix}.{ext}"));
    let with_caption_path = out_dir.join(format!("{base}_with-caption{quality_suffix}.{ext}"));

    // `out_dir` is created up front, but re-assert it here too: if this
    // export follows a (possibly long, network-bound) verification pass,
    // be defensive against anything having removed it out from under us in
    // the meantime, and give each write an explicit path in its error
    // context rather than a bare unlabelled io::Error.
    fs::create_dir_all(out_dir).with_context(|| format!("re-creating output dir {out_dir:?} before writing crops"))?;

    fs::write(&no_caption_path, encode(&no_caption_img, output_format)?)
        .with_context(|| format!("writing {no_caption_path:?}"))?;
    fs::write(&with_caption_path, encode(&with_caption_img, output_format)?)
        .with_context(|| format!("writing {with_caption_path:?}"))?;

    Ok(ExportedFiles {
        format: output_format.as_str().to_string(),
        with_caption: with_caption_path.to_string_lossy().to_string(),
        no_caption: no_caption_path.to_string_lossy().to_string(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jpegxl_quality_85_uses_libjxl_mapping() {
        assert!((jpegxl_distance_from_quality(85) - 1.45).abs() < f32::EPSILON);
    }

    #[test]
    fn jpegxl_encoder_produces_codestream() {
        let image = RgbImage::from_fn(4, 4, |x, y| {
            image::Rgb([(x * 40) as u8, (y * 40) as u8, ((x + y) * 20) as u8])
        });
        let encoded = encode_jpegxl(&image, QUALITY).expect("encode JPEG XL");
        assert!(encoded.starts_with(&[0xFF, 0x0A]));
    }
}
