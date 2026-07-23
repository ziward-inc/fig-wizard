//! Per-object export: render both the object-only and object+caption
//! crops (skipping the re-render when there's no caption), encode each in
//! the user-selected `OutputFormat` at quality 85 (or losslessly for PNG),
//! and write a `manifest.json` for the whole PDF.

use anyhow::{anyhow, Context, Result};
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageEncoder, RgbImage};
use pdfium_render::prelude::PdfPage;
use std::process::Command;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::pdf::render::{render_clip, ClipRenderBudget};
use crate::pipeline::types::{
    DetectedObject, ExportedFiles, Manifest, ManifestEntry, OutputFormat, VerificationInfo,
};
use crate::verify::run_with_timeout;

/// JPEG-style quality (0-100) applied to every lossy format. PNG is
/// lossless and ignores this.
const QUALITY: u8 = 85;

/// Wall-clock cap on a single `cjxl` invocation. `cjxl` on a near-4K crop
/// completes in well under a second in manual testing, so this leaves
/// generous headroom without letting a hung/stalled subprocess block the
/// pipeline indefinitely (mirrors `verify::CODEX_TIMEOUT_SECS`'s role for
/// the other subprocess this app shells out to).
const CJXL_TIMEOUT_SECS: u64 = 60;

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

/// JPEG XL encode at a fixed quality by shelling out to libjxl's `cjxl`
/// command-line encoder (see `pipeline::types::OutputFormat`'s doc comment
/// for why this is a subprocess rather than a linked-in crate). Writes
/// `img` to a temp PNG, invokes `cjxl <temp.png> <temp.jxl> -q QUALITY`,
/// reads back the resulting codestream, and cleans up both temp files
/// afterward regardless of success/failure.
///
/// Mirrors `verify::run_codex_verify`'s subprocess-handling style: a
/// dedicated temp working area, a hard timeout via `run_with_timeout` so a
/// hung `cjxl` can't block the pipeline forever, and stderr captured for a
/// clear error message on failure.
fn encode_jpegxl(img: &RgbImage, quality: u8) -> Result<Vec<u8>> {
    let work_dir = std::env::temp_dir().join(format!("pdf-extractor-jxl-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&work_dir)
        .with_context(|| format!("creating jxl temp work dir {work_dir:?}"))?;

    // Always clean up the temp dir on the way out, success or failure.
    let result = (|| -> Result<Vec<u8>> {
        let png_path = work_dir.join("input.png");
        let jxl_path = work_dir.join("output.jxl");

        fs::write(&png_path, encode_png(img)?)
            .with_context(|| format!("writing jxl temp input {png_path:?}"))?;

        let mut cmd = Command::new("cjxl");
        cmd.arg(&png_path).arg(&jxl_path).arg("-q").arg(quality.to_string());

        let output = run_with_timeout(cmd, Duration::from_secs(CJXL_TIMEOUT_SECS), false).map_err(|e| {
            anyhow!("running cjxl: {e}. Is libjxl installed? (`brew install jpeg-xl`)")
        })?;

        if !output.status.success() {
            anyhow::bail!(
                "cjxl exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }

        fs::read(&jxl_path).with_context(|| format!("reading cjxl output {jxl_path:?}"))
    })();

    let _ = fs::remove_dir_all(&work_dir);
    result
}

/// Quick check for whether the `cjxl` binary is callable at all (on `PATH`,
/// executes without erroring). Mirrors `verify::codex_available`'s
/// style/pattern exactly, so the app can preflight-check this before
/// starting an extraction run with JPEG XL selected, the same way Codex
/// availability is checked upfront for the verification feature.
pub fn cjxl_available() -> Result<String, String> {
    let mut cmd = Command::new("cjxl");
    cmd.arg("--version");
    match run_with_timeout(cmd, Duration::from_secs(10), true) {
        Ok(output) if output.status.success() => {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        }
        Ok(output) => Err(format!(
            "cjxl --version exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )),
        Err(e) => Err(format!("cjxl binary not runnable: {e}. Install via `brew install jpeg-xl`.")),
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
/// the caller-selected `output_format`, writing them under
/// `<page_dir>/<kind>-NN_{with,no}-caption[_q85].<ext>` (the `_q85` quality
/// suffix is omitted for PNG, since it's lossless).
pub fn export_object(
    page: &PdfPage,
    obj: &DetectedObject,
    page_dir: &Path,
    seq_in_page: u32,
    budget: ClipRenderBudget,
    output_format: OutputFormat,
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

    let ext = output_format.extension();
    let quality_suffix = if output_format.is_lossless() { "" } else { "_q85" };
    let no_caption_path = page_dir.join(format!("{base}_no-caption{quality_suffix}.{ext}"));
    let with_caption_path = page_dir.join(format!("{base}_with-caption{quality_suffix}.{ext}"));

    // `page_dir` is created up front, but re-assert it here too: if this
    // export follows a (possibly long, network-bound) verification pass,
    // be defensive against anything having removed it out from under us in
    // the meantime, and give each write an explicit path in its error
    // context rather than a bare unlabelled io::Error.
    fs::create_dir_all(page_dir).with_context(|| format!("re-creating page output dir {page_dir:?} before writing crops"))?;

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
