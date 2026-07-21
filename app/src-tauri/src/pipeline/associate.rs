//! Caption/number association.
//!
//! There is no per-object-type caption class in the model's label set:
//! `figure_title` is a single shared caption class used for figures,
//! tables, AND algorithms alike (visually confirmed in Phase 0). So
//! association is purely spatial: nearest `figure_title` box above/below
//! within a gap threshold, same-column, not-yet-claimed - never routed by
//! the caption box's own class name. `display_formula` objects pair
//! specifically with `formula_number` boxes instead (same line, to the
//! side, not below).
//!
//! If no caption/number box is found or associable for an object, the
//! object is exported with identical with/without-caption crops - this is
//! expected, not a bug.

use crate::pipeline::types::{BBoxPt, DetectedObject, Kind, PageDetection};

/// Maximum vertical gap (in points) between an object and a candidate
/// `figure_title` caption for them to be considered associable. Generous
/// enough to span a couple of line heights of whitespace, tight enough to
/// avoid grabbing an unrelated caption two paragraphs away.
const CAPTION_MAX_GAP_PT: f32 = 40.0;

/// How much a candidate caption/number box's horizontal extent must
/// overlap with the object's column (or vice versa) to count as
/// "same column". Purely area-based, so it doesn't depend on getting an
/// exact 1-vs-2-column boundary right.
const MIN_COLUMN_OVERLAP_FRACTION: f32 = 0.25;

/// Classes never extracted as standalone objects and never treated as
/// column-layout signal either (headers/footers/page numbers routinely
/// span oddly and would corrupt column-gap detection).
const COLUMN_LAYOUT_EXCLUDED: &[&str] = &[
    "header",
    "footer",
    "header_image",
    "footer_image",
    "number",
    "doc_title",
    "abstract",
];

/// Detects a two-column layout by looking for a clean horizontal gap
/// between a left cluster and a right cluster of (reasonably narrow) page
/// boxes. Returns `Some(boundary_x_pt)` if a two-column layout is detected,
/// `None` for single-column (or anything ambiguous, which we treat as
/// single-column - the x-overlap fallback in `same_column` still works
/// fine in that case).
fn detect_column_boundary(dets: &[PageDetection], page_width_pt: f32) -> Option<f32> {
    let narrow_max_width = page_width_pt * 0.55;

    let narrow: Vec<&BBoxPt> = dets
        .iter()
        .filter(|d| !COLUMN_LAYOUT_EXCLUDED.contains(&d.label.as_str()))
        .map(|d| &d.bbox_pt)
        .filter(|b| b.width() < narrow_max_width)
        .collect();

    if narrow.len() < 4 {
        return None;
    }

    let mid = page_width_pt / 2.0;
    let left: Vec<&&BBoxPt> = narrow.iter().filter(|b| b.x_center() < mid).collect();
    let right: Vec<&&BBoxPt> = narrow.iter().filter(|b| b.x_center() >= mid).collect();

    if left.len() < 2 || right.len() < 2 {
        return None;
    }

    let left_max_x1 = left.iter().map(|b| b.x1).fold(f32::MIN, f32::max);
    let right_min_x0 = right.iter().map(|b| b.x0).fold(f32::MAX, f32::min);

    let gap = right_min_x0 - left_max_x1;
    if gap > page_width_pt * 0.03 && gap < page_width_pt * 0.3 {
        Some((left_max_x1 + right_min_x0) / 2.0)
    } else {
        None
    }
}

fn column_of(x_center: f32, boundary: Option<f32>) -> u8 {
    match boundary {
        Some(b) => {
            if x_center < b {
                0
            } else {
                1
            }
        }
        None => 0,
    }
}

fn same_column(a: &BBoxPt, b: &BBoxPt, boundary: Option<f32>) -> bool {
    if column_of(a.x_center(), boundary) == column_of(b.x_center(), boundary) {
        return true;
    }
    // Fall back to raw overlap so wide (e.g. double-column-spanning)
    // figures/tables still match captions even if their x-centers land on
    // opposite sides of the detected boundary.
    a.x_overlap_fraction(b) > MIN_COLUMN_OVERLAP_FRACTION || b.x_overlap_fraction(a) > MIN_COLUMN_OVERLAP_FRACTION
}

/// Vertical gap (points) between two non-overlapping boxes, or `None` if
/// they overlap vertically. Direction-agnostic: works whether `b` is above
/// or below `a`.
fn vertical_gap(a: &BBoxPt, b: &BBoxPt) -> Option<f32> {
    if b.y1 <= a.y0 {
        Some(a.y0 - b.y1) // b below a
    } else if b.y0 >= a.y1 {
        Some(b.y0 - a.y1) // b above a
    } else {
        None // vertical overlap - not a valid caption position
    }
}

/// For formula/formula_number pairing: the number must sit on (approximately)
/// the same line as the formula, to one side of it (never above/below).
/// Returns the horizontal gap if so.
fn same_line_horizontal_gap(formula: &BBoxPt, number: &BBoxPt) -> Option<f32> {
    let tolerance = formula.height() * 0.4;
    let number_center = number.y_center();
    if number_center < formula.y0 - tolerance || number_center > formula.y1 + tolerance {
        return None;
    }
    if number.x0 >= formula.x1 {
        Some(number.x0 - formula.x1)
    } else if number.x1 <= formula.x0 {
        Some(formula.x0 - number.x1)
    } else {
        None // horizontally overlapping - not a side placement
    }
}

struct CandidatePair {
    object_idx: usize,
    pool_idx: usize,
    distance: f32,
}

/// Greedily assigns each object at most one caption/number box, and each
/// caption/number box to at most one object, preferring globally-shortest
/// distances first (rather than a naive per-object nearest-neighbor scan,
/// which can let an earlier object "steal" the best match for a later one).
fn greedy_match(mut pairs: Vec<CandidatePair>, n_objects: usize, n_pool: usize) -> Vec<Option<usize>> {
    pairs.sort_by(|a, b| a.distance.partial_cmp(&b.distance).unwrap());
    let mut claimed_pool = vec![false; n_pool];
    let mut assigned = vec![None; n_objects];
    for pair in pairs {
        if claimed_pool[pair.pool_idx] || assigned[pair.object_idx].is_some() {
            continue;
        }
        claimed_pool[pair.pool_idx] = true;
        assigned[pair.object_idx] = Some(pair.pool_idx);
    }
    assigned
}

/// Runs class-filtering + caption/number association for one page's worth
/// of detections (already in PDF point space). Returns the final list of
/// extractable objects with captions associated where possible.
pub fn associate_page(page_index: u32, dets: &[PageDetection], page_width_pt: f32) -> Vec<DetectedObject> {
    let boundary = detect_column_boundary(dets, page_width_pt);

    // Split into: extractable objects (by Kind), figure_title pool, and
    // formula_number pool. Everything else is ignored for association
    // purposes (still useful for column detection above, but not a
    // caption/number candidate or an extractable object).
    struct ObjSlot<'a> {
        det: &'a PageDetection,
        kind: Kind,
    }
    let mut objects: Vec<ObjSlot> = Vec::new();
    let mut title_pool: Vec<&PageDetection> = Vec::new();
    let mut number_pool: Vec<&PageDetection> = Vec::new();

    for det in dets {
        if let Some(kind) = Kind::from_label(&det.label) {
            objects.push(ObjSlot { det, kind });
        } else if det.label == "figure_title" {
            title_pool.push(det);
        } else if det.label == "formula_number" {
            number_pool.push(det);
        }
    }

    // --- formula <-> formula_number association ---
    let formula_pairs: Vec<CandidatePair> = objects
        .iter()
        .enumerate()
        .filter(|(_, o)| o.kind == Kind::Formula)
        .flat_map(|(oi, o)| {
            number_pool.iter().enumerate().filter_map(move |(ni, n)| {
                same_line_horizontal_gap(&o.det.bbox_pt, &n.bbox_pt).map(|d| CandidatePair {
                    object_idx: oi,
                    pool_idx: ni,
                    distance: d,
                })
            })
        })
        .collect();
    let formula_assignment = greedy_match(formula_pairs, objects.len(), number_pool.len());

    // --- everything else <-> figure_title association ---
    let title_pairs: Vec<CandidatePair> = objects
        .iter()
        .enumerate()
        .filter(|(_, o)| o.kind != Kind::Formula)
        .flat_map(|(oi, o)| {
            title_pool.iter().enumerate().filter_map(move |(ti, t)| {
                if !same_column(&o.det.bbox_pt, &t.bbox_pt, boundary) {
                    return None;
                }
                vertical_gap(&o.det.bbox_pt, &t.bbox_pt).and_then(|d| {
                    if d <= CAPTION_MAX_GAP_PT {
                        Some(CandidatePair {
                            object_idx: oi,
                            pool_idx: ti,
                            distance: d,
                        })
                    } else {
                        None
                    }
                })
            })
        })
        .collect();
    let title_assignment = greedy_match(title_pairs, objects.len(), title_pool.len());

    let mut result = Vec::with_capacity(objects.len());
    let mut counters: std::collections::HashMap<&'static str, u32> = std::collections::HashMap::new();

    for (oi, slot) in objects.iter().enumerate() {
        let caption_bbox_pt = if slot.kind == Kind::Formula {
            formula_assignment[oi].map(|ni| number_pool[ni].bbox_pt)
        } else {
            title_assignment[oi].map(|ti| title_pool[ti].bbox_pt)
        };

        let n = counters.entry(slot.kind.as_str()).or_insert(0);
        *n += 1;
        let id = format!("p{:04}-{}-{:02}", page_index, slot.kind.as_str(), n);

        result.push(DetectedObject {
            id,
            kind: slot.kind,
            raw_label: slot.det.label.clone(),
            score: slot.det.score,
            page_index,
            bbox_pt: slot.det.bbox_pt,
            caption_bbox_pt,
        });
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn det(label: &str, x0: f32, y0: f32, x1: f32, y1: f32) -> PageDetection {
        PageDetection {
            label: label.to_string(),
            score: 0.9,
            bbox_pt: BBoxPt { x0, y0, x1, y1 },
        }
    }

    #[test]
    fn figure_gets_caption_below() {
        // PDF y-up: figure sits above its caption (larger y).
        let figure = det("image", 50.0, 400.0, 300.0, 600.0);
        let caption = det("figure_title", 50.0, 370.0, 300.0, 395.0);
        let dets = vec![figure, caption];
        let objs = associate_page(0, &dets, 612.0);
        assert_eq!(objs.len(), 1);
        assert!(objs[0].caption_bbox_pt.is_some());
    }

    #[test]
    fn caption_too_far_is_not_associated() {
        let figure = det("image", 50.0, 400.0, 300.0, 600.0);
        let far_caption = det("figure_title", 50.0, 100.0, 300.0, 120.0);
        let dets = vec![figure, far_caption];
        let objs = associate_page(0, &dets, 612.0);
        assert_eq!(objs.len(), 1);
        assert!(objs[0].caption_bbox_pt.is_none());
    }

    #[test]
    fn formula_pairs_with_side_number_not_below_text() {
        let formula = det("display_formula", 100.0, 500.0, 400.0, 530.0);
        let number = det("formula_number", 480.0, 505.0, 510.0, 525.0); // same line, to the right
        let below_text = det("text", 100.0, 460.0, 400.0, 490.0); // decoy below
        let dets = vec![formula, number, below_text];
        let objs = associate_page(0, &dets, 612.0);
        assert_eq!(objs.len(), 1);
        assert_eq!(objs[0].kind, Kind::Formula);
        assert!(objs[0].caption_bbox_pt.is_some());
    }

    #[test]
    fn two_column_layout_does_not_cross_associate() {
        // Left column figure with a caption in the right column at the
        // same height should NOT be associated; only same-column caption
        // (directly below, in the left column) should match.
        let page_width = 600.0;
        let mut dets = vec![
            det("image", 50.0, 400.0, 250.0, 600.0),
            det("figure_title", 320.0, 400.0, 550.0, 420.0), // right column, decoy
            det("figure_title", 50.0, 370.0, 250.0, 395.0),  // left column, correct
        ];
        // Populate enough narrow text boxes on both sides to make column
        // detection kick in.
        for i in 0..3 {
            let y = 100.0 + i as f32 * 40.0;
            dets.push(det("text", 50.0, y, 250.0, y + 30.0));
            dets.push(det("text", 320.0, y, 550.0, y + 30.0));
        }
        let objs = associate_page(0, &dets, page_width);
        let figure = objs.iter().find(|o| o.kind == Kind::Figure).unwrap();
        let cap = figure.caption_bbox_pt.unwrap();
        assert!((cap.x0 - 50.0).abs() < 1.0, "expected left-column caption to be chosen");
    }
}
