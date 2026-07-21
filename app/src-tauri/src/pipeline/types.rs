//! Shared data types for the detection -> association -> crop -> export pipeline.

use serde::Serialize;

/// An axis-aligned box in PDF point space (1/72 inch), using PDF's native
/// bottom-left origin / y-up convention. This is the canonical coordinate
/// space we store detections in once they leave the pixel space of any
/// particular raster render.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct BBoxPt {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl BBoxPt {
    pub fn width(&self) -> f32 {
        self.x1 - self.x0
    }

    pub fn height(&self) -> f32 {
        self.y1 - self.y0
    }

    pub fn x_center(&self) -> f32 {
        (self.x0 + self.x1) / 2.0
    }

    pub fn y_center(&self) -> f32 {
        (self.y0 + self.y1) / 2.0
    }

    /// Union (bounding box) of two boxes.
    pub fn union(&self, other: &BBoxPt) -> BBoxPt {
        BBoxPt {
            x0: self.x0.min(other.x0),
            y0: self.y0.min(other.y0),
            x1: self.x1.max(other.x1),
            y1: self.y1.max(other.y1),
        }
    }

    /// Horizontal overlap fraction relative to the narrower of the two boxes,
    /// used for "same column" heuristics.
    pub fn x_overlap_fraction(&self, other: &BBoxPt) -> f32 {
        let overlap = (self.x1.min(other.x1) - self.x0.max(other.x0)).max(0.0);
        let narrower = self.width().min(other.width()).max(1.0);
        overlap / narrower
    }
}

/// One page-level detection already converted into PDF point space, with
/// its raw model label preserved (used to route into the exclusion/
/// candidate/caption-pool logic in `pipeline::associate`).
#[derive(Debug, Clone)]
pub struct PageDetection {
    pub label: String,
    pub score: f32,
    pub bbox_pt: BBoxPt,
}

/// A raw detection straight out of the ONNX model, in the pixel space of
/// whatever page raster was fed to the detector (top-left origin, y-down).
#[derive(Debug, Clone)]
pub struct RawDetection {
    pub label_idx: usize,
    pub label: String,
    pub score: f32,
    /// Pixel-space box, top-left origin, y-down, in the coordinate space of
    /// the detection-pass render (see `pdf::render::render_page_for_detection`).
    pub px_x0: f32,
    pub px_y0: f32,
    pub px_x1: f32,
    pub px_y1: f32,
}

/// The kind of extractable object we export, after collapsing the raw model
/// classes down per the product spec (chart/image -> figure, etc).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Figure,
    Table,
    Formula,
    Algorithm,
    Aside,
    Seal,
}

impl Kind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Kind::Figure => "figure",
            Kind::Table => "table",
            Kind::Formula => "formula",
            Kind::Algorithm => "algorithm",
            Kind::Aside => "aside",
            Kind::Seal => "seal",
        }
    }

    /// Maps a raw model class name to an extractable `Kind`, if it is a
    /// candidate class at all. Returns `None` for excluded/non-extractable
    /// classes and for the two "helper" classes (`figure_title`,
    /// `formula_number`) that are consumed during association rather than
    /// exported as standalone objects.
    pub fn from_label(label: &str) -> Option<Kind> {
        match label {
            "chart" | "image" => Some(Kind::Figure),
            "table" => Some(Kind::Table),
            "display_formula" => Some(Kind::Formula),
            "algorithm" => Some(Kind::Algorithm),
            "aside_text" => Some(Kind::Aside),
            "seal" => Some(Kind::Seal),
            _ => None,
        }
    }
}

/// A detected object that survived class-filtering, in PDF point space,
/// with an optional associated caption/number box (also in point space).
#[derive(Debug, Clone)]
pub struct DetectedObject {
    pub id: String,
    pub kind: Kind,
    pub raw_label: String,
    pub score: f32,
    pub page_index: u32,
    /// The object's own detected box.
    pub bbox_pt: BBoxPt,
    /// Associated caption (`figure_title`) or formula number
    /// (`formula_number`) box, if one was found and claimed.
    pub caption_bbox_pt: Option<BBoxPt>,
}

impl DetectedObject {
    /// The box used for the "with caption" crop: union of object + caption
    /// if a caption was associated, else identical to the object-only box.
    pub fn with_caption_bbox(&self) -> BBoxPt {
        match self.caption_bbox_pt {
            Some(cap) => self.bbox_pt.union(&cap),
            None => self.bbox_pt,
        }
    }
}

/// Paths to the four exported image variants for one detected object.
#[derive(Debug, Clone, Serialize)]
pub struct ExportedFiles {
    pub with_caption_webp: String,
    pub no_caption_webp: String,
    pub with_caption_avif: String,
    pub no_caption_avif: String,
}

/// One manifest entry: everything an external tool/reviewer needs to know
/// about one exported object.
#[derive(Debug, Clone, Serialize)]
pub struct ManifestEntry {
    pub id: String,
    pub kind: String,
    pub raw_label: String,
    pub page_index: u32,
    pub score: f32,
    pub bbox_pt: [f32; 4],
    pub with_caption_bbox_pt: [f32; 4],
    pub has_caption: bool,
    pub files: ExportedFiles,
}

#[derive(Debug, Clone, Serialize)]
pub struct Manifest {
    pub source_pdf: String,
    pub page_count: u32,
    pub objects: Vec<ManifestEntry>,
}
