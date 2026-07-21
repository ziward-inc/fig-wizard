//! PDF rendering via pdfium-render, two passes:
//!
//! 1. A modest-DPI full-page render used purely for layout detection
//!    (`render_page_for_detection`), plus a non-uniform resize to the
//!    model's fixed 800x800 input (`resize_for_model`), replicating the
//!    exact Phase-0 preprocessing recipe in `phase0-spike/detect.py`.
//! 2. A per-object high-DPI clip render (`render_clip`) that rasterizes
//!    *only* the detected object's bounding box directly from PDF vector
//!    content, at whatever DPI is needed to hit ~4K on the long side. This
//!    never renders (or upscales from) a full-page raster - see the
//!    `set_origin` + undersized-bitmap trick below, which is exactly the
//!    pattern pdfium-render's own docs recommend for rendering a partial
//!    region of a page without paying for the whole page.
//!
//! Coordinate systems: PDF point space is bottom-left origin, y-up.
//! Rendered bitmap pixel space is top-left origin, y-down. Every
//! conversion between the two is explicit here (`pixel_box_to_pdf_points`
//! / `pdf_points_to_pixel_box`) and covered by round-trip unit tests below,
//! since getting this backwards is the classic way to silently crop the
//! wrong part of the page.

use anyhow::{anyhow, Context, Result};
use image::{imageops::FilterType, RgbImage};
use pdfium_render::prelude::*;
use std::path::Path;

use crate::pipeline::types::BBoxPt;

/// Load the Pdfium library from a directory containing the platform dylib
/// (e.g. `libpdfium.dylib` on macOS). During development this points at
/// `src-tauri/binaries/pdfium/lib`; in production this should point at a
/// location populated by a first-run download step (not implemented yet -
/// see `pdfium_source` note in `commands.rs`).
pub fn init_pdfium(lib_dir: &Path) -> Result<Pdfium> {
    let lib_path = Pdfium::pdfium_platform_library_name_at_path(lib_dir);
    let bindings = Pdfium::bind_to_library(&lib_path)
        .with_context(|| format!("failed to load pdfium library at {lib_path:?}"))?;
    Ok(Pdfium::new(bindings))
}

#[derive(Debug, Clone, Copy)]
pub struct PageGeometry {
    pub width_pt: f32,
    pub height_pt: f32,
}

pub fn page_geometry(page: &PdfPage) -> PageGeometry {
    PageGeometry {
        width_pt: page.width().value,
        height_pt: page.height().value,
    }
}

/// Renders the full page at a fixed DPI (aspect ratio preserved), suitable
/// as the "original image" input to `resize_for_model`. Returns the
/// rendered RGB image plus the page geometry (in points) needed for
/// pixel<->point conversions.
pub fn render_page_for_detection(page: &PdfPage, detect_dpi: f32) -> Result<(RgbImage, PageGeometry)> {
    let geometry = page_geometry(page);
    let scale = detect_dpi / 72.0;
    let width_px = (geometry.width_pt * scale).round().max(1.0) as u16;
    let height_px = (geometry.height_pt * scale).round().max(1.0) as u16;

    let config = PdfRenderConfig::new()
        .set_target_size(width_px as i32, height_px as i32)
        .render_annotations(false);

    let bitmap = page
        .render_with_config(&config)
        .map_err(|e| anyhow!("pdfium render_with_config failed: {e}"))?;

    let dynamic = bitmap
        .as_image()
        .map_err(|e| anyhow!("pdfium bitmap as_image failed: {e}"))?;

    Ok((dynamic.to_rgb8(), geometry))
}

/// Non-uniform resize (aspect ratio NOT preserved - matches `keep_ratio:
/// false` in the model's Resize preprocessing step) to the model's fixed
/// input size. Returns the resized image plus `(scale_h, scale_w) =
/// (target/orig)`, which is exactly the `scale_factor` model input.
pub fn resize_for_model(img: &RgbImage, target_w: u32, target_h: u32) -> (RgbImage, f32, f32) {
    let (orig_w, orig_h) = (img.width(), img.height());
    // `Triangle` is the standard bilinear-equivalent filter, matching
    // cv2.INTER_LINEAR closely (see detect.py's preprocess() docstring for
    // why plain bilinear /255 scaling, no mean/std normalization, is the
    // validated recipe).
    let resized = image::imageops::resize(img, target_w, target_h, FilterType::Triangle);
    let scale_h = target_h as f32 / orig_h as f32;
    let scale_w = target_w as f32 / orig_w as f32;
    (resized, scale_h, scale_w)
}

/// Converts a pixel-space box (top-left origin, y-down, rendered at `dpi`)
/// into PDF point space (bottom-left origin, y-up, `page_height_pt` tall).
pub fn pixel_box_to_pdf_points(
    px_x0: f32,
    px_y0: f32,
    px_x1: f32,
    px_y1: f32,
    dpi: f32,
    page_height_pt: f32,
) -> BBoxPt {
    let pt_per_px = 72.0 / dpi;
    let x0 = px_x0 * pt_per_px;
    let x1 = px_x1 * pt_per_px;
    // px_y0 is the box's top edge (smaller pixel y => higher on the page =>
    // larger PDF y); px_y1 is the bottom edge (larger pixel y => smaller
    // PDF y). Flip explicitly so BBoxPt.y0 < y1 always holds.
    let y1 = page_height_pt - px_y0 * pt_per_px;
    let y0 = page_height_pt - px_y1 * pt_per_px;
    BBoxPt { x0, y0, x1, y1 }
}

/// Inverse of `pixel_box_to_pdf_points`: converts a PDF-point-space box back
/// into pixel space (top-left origin, y-down) at the given `dpi`.
pub fn pdf_points_to_pixel_box(
    bbox: BBoxPt,
    dpi: f32,
    page_height_pt: f32,
) -> (f32, f32, f32, f32) {
    let px_per_pt = dpi / 72.0;
    let px_x0 = bbox.x0 * px_per_pt;
    let px_x1 = bbox.x1 * px_per_pt;
    let px_y0 = (page_height_pt - bbox.y1) * px_per_pt;
    let px_y1 = (page_height_pt - bbox.y0) * px_per_pt;
    (px_x0, px_y0, px_x1, px_y1)
}

/// Rendering budget/safety-rail configuration for `render_clip`.
#[derive(Debug, Clone, Copy)]
pub struct ClipRenderBudget {
    /// Target size, in pixels, for the long side of the crop (~3840 for
    /// near-4K).
    pub target_long_side_px: f32,
    /// DPI ceiling used purely as a safety rail against degenerate tiny
    /// boxes that would otherwise imply an absurd DPI (e.g. a
    /// mis-detected 2pt-tall box). This is intentionally high (1200-1500)
    /// so that normal single-column figures are never DPI-capped down to
    /// a low pixel count - the real safety guard is `max_long_side_px`.
    pub dpi_ceiling: f32,
    /// Absolute pixel cap on the long side, guarding against degenerate
    /// oversized/misdetected boxes (e.g. a box that's nearly the whole
    /// page) blowing up render time/memory.
    pub max_long_side_px: f32,
}

impl Default for ClipRenderBudget {
    fn default() -> Self {
        ClipRenderBudget {
            target_long_side_px: 3840.0,
            dpi_ceiling: 1400.0,
            max_long_side_px: 6000.0,
        }
    }
}

/// Renders only the given PDF-point-space clip rectangle, directly from
/// vector content, at whatever DPI is needed to hit `target_long_side_px`
/// on the clip's long side (subject to the DPI ceiling and absolute pixel
/// cap in `budget`). Never renders a full page and crops - see module docs
/// for the `set_origin` + undersized-bitmap technique used here.
pub fn render_clip(
    page: &PdfPage,
    clip_pt: BBoxPt,
    budget: ClipRenderBudget,
) -> Result<RgbImage> {
    let geometry = page_geometry(page);

    let clip_w_pt = clip_pt.width().max(0.1);
    let clip_h_pt = clip_pt.height().max(0.1);
    let long_side_pt = clip_w_pt.max(clip_h_pt);

    let mut scale = budget.target_long_side_px / long_side_pt;
    let dpi = (scale * 72.0).min(budget.dpi_ceiling);
    scale = dpi / 72.0;

    let mut clip_w_px = (clip_w_pt * scale).round().max(1.0);
    let mut clip_h_px = (clip_h_pt * scale).round().max(1.0);

    // Absolute pixel cap as the real safety guard (applied uniformly to
    // preserve aspect ratio).
    let long_side_px = clip_w_px.max(clip_h_px);
    if long_side_px > budget.max_long_side_px {
        let shrink = budget.max_long_side_px / long_side_px;
        clip_w_px = (clip_w_px * shrink).round().max(1.0);
        clip_h_px = (clip_h_px * shrink).round().max(1.0);
        scale *= shrink;
    }

    // Full-page scaled size at this same scale factor - used only so
    // Pdfium can compute the correct page->device transform; we do NOT
    // allocate a bitmap this large. Pdfium clips rendering to the
    // destination bitmap's actual (small) size, which is exactly the
    // pattern documented on `PdfRenderConfig::set_origin`.
    let page_w_px = (geometry.width_pt * scale).round().max(1.0);
    let page_h_px = (geometry.height_pt * scale).round().max(1.0);

    // Pixel-space top-left origin of the clip box (top-left, y-down) at
    // this scale, via the same explicit flip used everywhere else.
    let (px_x0, px_y0, _px_x1, _px_y1) = pdf_points_to_pixel_box(clip_pt, dpi, geometry.height_pt);

    let bitmap_w = clip_w_px as i32;
    let bitmap_h = clip_h_px as i32;

    let mut bitmap = PdfBitmap::empty(bitmap_w, bitmap_h, PdfBitmapFormat::default())
        .map_err(|e| anyhow!("failed to allocate clip bitmap: {e}"))?;

    let config = PdfRenderConfig::new()
        .set_target_size(page_w_px as i32, page_h_px as i32)
        .set_origin(-px_x0.round() as i32, -px_y0.round() as i32)
        .render_annotations(false);

    page.render_into_bitmap_with_config(&mut bitmap, &config)
        .map_err(|e| anyhow!("pdfium render_into_bitmap_with_config failed: {e}"))?;

    let dynamic = bitmap
        .as_image()
        .map_err(|e| anyhow!("pdfium bitmap as_image failed: {e}"))?;

    Ok(dynamic.to_rgb8())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_to_point_round_trip() {
        let page_height_pt = 792.0; // US Letter
        let dpi = 200.0;

        let (px_x0, px_y0, px_x1, px_y1) = (100.0_f32, 50.0_f32, 300.0_f32, 150.0_f32);

        let bbox = pixel_box_to_pdf_points(px_x0, px_y0, px_x1, px_y1, dpi, page_height_pt);

        // Known-value check: at 200 DPI, 1px = 0.36pt.
        assert!((bbox.x0 - 36.0).abs() < 1e-3);
        assert!((bbox.x1 - 108.0).abs() < 1e-3);
        // Top edge (px_y0=50) is farther from the bottom than the bottom
        // edge (px_y1=150), so it must map to the LARGER pdf y (y1).
        assert!((bbox.y1 - 774.0).abs() < 1e-3);
        assert!((bbox.y0 - 738.0).abs() < 1e-3);
        assert!(bbox.y0 < bbox.y1, "y0 must stay below y1 after the flip");

        let (rx0, ry0, rx1, ry1) = pdf_points_to_pixel_box(bbox, dpi, page_height_pt);
        assert!((rx0 - px_x0).abs() < 1e-2);
        assert!((ry0 - px_y0).abs() < 1e-2);
        assert!((rx1 - px_x1).abs() < 1e-2);
        assert!((ry1 - px_y1).abs() < 1e-2);
    }

    #[test]
    fn box_near_top_of_page_has_high_pdf_y() {
        // A box near the visual top of the page should end up with y close
        // to page_height_pt (since PDF y-up puts high values at the top).
        let page_height_pt = 792.0;
        let dpi = 200.0;
        let bbox = pixel_box_to_pdf_points(0.0, 0.0, 100.0, 20.0, dpi, page_height_pt);
        assert!(bbox.y1 > 700.0, "expected near-top box to have high pdf y1, got {}", bbox.y1);
    }

    #[test]
    fn box_near_bottom_of_page_has_low_pdf_y() {
        let page_height_pt = 792.0;
        let dpi = 200.0;
        // At 200dpi, page height in px = 792/72*200 = 2200.
        let bbox = pixel_box_to_pdf_points(0.0, 2180.0, 100.0, 2200.0, dpi, page_height_pt);
        assert!(bbox.y0 < 10.0, "expected near-bottom box to have low pdf y0, got {}", bbox.y0);
    }
}
