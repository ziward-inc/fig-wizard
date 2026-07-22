//! Optional, off-by-default runtime crop verification: shells out to the
//! `codex` CLI (OpenAI's coding agent, used here purely as a multimodal
//! judge - no code editing happens) with the rendered crop image and asks
//! it to judge whether the crop is a clean, complete, standalone image of
//! the detected object. If not, Codex proposes a bounding-box correction
//! (in PDF points, on the visual sides of the image); we apply it, re-render,
//! and re-verify, up to `MAX_ATTEMPTS` total tries.
//!
//! This module is purely additive: any failure to invoke/parse Codex
//! (binary missing, auth error, timeout, malformed JSON) is treated as a
//! *soft* failure - it consumes an attempt and is recorded (both in
//! `VerifyOutcome::last_issue` and as its own entry in
//! `VerifyOutcome::history`) as `"verification_error: ..."`, but never
//! propagates as a hard `Err` that would abort the whole extraction job.
//! The only `Err` this module returns is for a real rendering failure
//! (`pdf::render::render_clip`), which would fail the export anyway.
//!
//! Every attempt - passed, corrected-and-retried, soft-failed, or
//! cancelled - gets one entry in `VerifyOutcome::history`, so callers (the
//! manifest, the results-gallery modal) can see exactly what happened on
//! every retry, not just the final one.
//!
//! Sign convention for `bbox_adjustment_pt` (mirrors the explicit PDF-point
//! Y-flip convention already established in `pdf::render` /
//! `pipeline::types::BBoxPt`): fields are named for the VISUAL side of the
//! rendered crop (top = upper edge as a human sees it). A POSITIVE value on
//! a side means "expand the crop outward on that side" (reveal more
//! content); a NEGATIVE value means "shrink the crop inward on that side"
//! (remove content). Since `BBoxPt` is PDF point space (bottom-left origin,
//! y-up - see `pdf::render` module docs), the visual top edge is the box's
//! `y1` and the visual bottom edge is its `y0`, so:
//!   new_x0 = x0 - left
//!   new_x1 = x1 + right
//!   new_y0 = y0 - bottom
//!   new_y1 = y1 + top
//! (expanding "top" increases y1; expanding "bottom" decreases y0). See the
//! unit tests below for worked examples.

use anyhow::{Context, Result};
use image::RgbImage;
use pdfium_render::prelude::PdfPage;
use serde::Deserialize;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::pdf::render::{page_geometry, render_clip, ClipRenderBudget};
use crate::pipeline::types::{BBoxPt, VerificationAttempt};

/// Total attempts per object: 1 initial check + up to 2 corrective
/// re-renders. Named/exported so it's easy to find and tune.
pub const MAX_ATTEMPTS: u32 = 3;

/// Wall-clock cap on a single `codex exec` invocation. A manual timing run
/// against a real crop (see dev notes / README) took ~8s at default
/// reasoning effort; this leaves generous headroom for slower prompts
/// without letting a hung/stalled call block the pipeline indefinitely.
const CODEX_TIMEOUT_SECS: u64 = 90;

/// Absolute cap (in PDF points) on how far a single corrective adjustment
/// may move any one side, guarding against a wild/degenerate Codex
/// suggestion blowing up the crop.
const MAX_ADJUSTMENT_PT: f32 = 200.0;

/// Floor (in PDF points) under which we refuse to let width/height shrink,
/// guarding against a shrink-adjustment collapsing or inverting the box.
const MIN_DIM_PT: f32 = 5.0;

const VERIFY_SCHEMA_JSON: &str = r#"{
  "type": "object",
  "properties": {
    "passed": { "type": "boolean" },
    "issue": {
      "type": "string",
      "enum": [
        "none",
        "top_cut_off",
        "bottom_cut_off",
        "left_cut_off",
        "right_cut_off",
        "extra_content_top",
        "extra_content_bottom",
        "extra_content_left",
        "extra_content_right",
        "wrong_content"
      ]
    },
    "bbox_adjustment_pt": {
      "type": "object",
      "properties": {
        "top": { "type": "number" },
        "bottom": { "type": "number" },
        "left": { "type": "number" },
        "right": { "type": "number" }
      },
      "required": ["top", "bottom", "left", "right"],
      "additionalProperties": false
    },
    "reason": { "type": "string" }
  },
  "required": ["passed", "issue", "bbox_adjustment_pt", "reason"],
  "additionalProperties": false
}
"#;

/// Result of running `verify_and_correct_crop` for one object: how many
/// attempts it took, whether it ultimately passed, the last-seen
/// issue/reason (useful for both the manifest and debugging), and the full
/// per-attempt `history` (one entry per real attempt, in order - see
/// `VerificationAttempt`). `attempts`/`last_issue`/`last_reason` are
/// convenience fields derived from `history` (`attempts == history.len()`,
/// `last_issue`/`last_reason` mirror `history.last()`) so existing call
/// sites that only care about the summary don't need to touch the vec.
#[derive(Debug, Clone)]
pub struct VerifyOutcome {
    pub passed: bool,
    pub attempts: u32,
    pub last_issue: Option<String>,
    pub last_reason: Option<String>,
    pub history: Vec<VerificationAttempt>,
}

/// Builds a `VerifyOutcome` from an in-progress `history` vec, deriving
/// `attempts`/`last_issue`/`last_reason` from it so every return path stays
/// consistent (see `VerifyOutcome` doc comment).
fn finish_outcome(history: Vec<VerificationAttempt>, passed: bool) -> VerifyOutcome {
    let attempts = history.len() as u32;
    let last_issue = history.last().map(|a| a.issue.clone());
    let last_reason = history.last().map(|a| a.reason.clone());
    VerifyOutcome { passed, attempts, last_issue, last_reason, history }
}

/// One parsed Codex response, matching `VERIFY_SCHEMA_JSON`.
#[derive(Debug, Clone, Deserialize)]
struct VerifyResult {
    passed: bool,
    issue: String,
    bbox_adjustment_pt: BboxAdjustment,
    reason: String,
}

#[derive(Debug, Clone, Deserialize)]
struct BboxAdjustment {
    top: f32,
    bottom: f32,
    left: f32,
    right: f32,
}

/// Quick check for whether the `codex` binary is callable at all (on
/// `PATH`, executes without erroring). Used both as an upfront preflight
/// before starting a whole extraction run with verification enabled, and is
/// safe to call cheaply/often (it just runs `codex --version`).
pub fn codex_available() -> Result<String, String> {
    let mut cmd = Command::new("codex");
    cmd.arg("--version");
    match run_with_timeout(cmd, Duration::from_secs(10), true) {
        Ok(output) if output.status.success() => {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        }
        Ok(output) => Err(format!(
            "codex --version exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )),
        Err(e) => Err(format!("codex binary not runnable: {e}")),
    }
}

/// Renders `initial_bbox_pt`, asks Codex to verify it, and if it fails,
/// applies Codex's suggested correction and retries - up to `max_attempts`
/// total tries. Always returns `Ok` unless the underlying PDF render itself
/// fails (which would fail the export anyway); any Codex/process/parse
/// failure is captured as a soft failure inside `VerifyOutcome` instead.
///
/// `cancel` is polled between attempts (in addition to the between-object
/// check already done by the caller) so a multi-attempt verification loop
/// doesn't make Cancel unresponsive.
pub fn verify_and_correct_crop(
    page: &PdfPage,
    kind: &str,
    initial_bbox_pt: BBoxPt,
    budget: ClipRenderBudget,
    max_attempts: u32,
    work_dir: &Path,
    cancel: &AtomicBool,
) -> Result<(RgbImage, BBoxPt, VerifyOutcome)> {
    std::fs::create_dir_all(work_dir)
        .with_context(|| format!("creating verify work dir {work_dir:?}"))?;

    let schema_path = work_dir.join("verify_schema.json");
    std::fs::write(&schema_path, VERIFY_SCHEMA_JSON)
        .with_context(|| format!("writing {schema_path:?}"))?;

    let geometry = page_geometry(page);
    let mut current_bbox = initial_bbox_pt;
    let mut history: Vec<VerificationAttempt> = Vec::new();

    let max_attempts = max_attempts.max(1);

    for attempt in 1..=max_attempts {
        if cancel.load(Ordering::Relaxed) {
            // Bail out of the verify loop early; report what we have as an
            // unverified (not-passed) result rather than block cancellation.
            // No real attempt ran this iteration (the check happens before
            // any render/Codex call), but we still record a history entry
            // so a manifest reader can see that cancellation - not a Codex
            // verdict - is why the loop stopped here.
            let img = render_clip(page, current_bbox, budget)
                .with_context(|| "rendering clip after cancellation during verify")?;
            history.push(VerificationAttempt {
                attempt,
                passed: false,
                issue: "cancelled".to_string(),
                reason: "extraction was cancelled before this verification attempt ran".to_string(),
                bbox_adjustment_pt: None,
            });
            return Ok((img, current_bbox, finish_outcome(history, false)));
        }

        let img = render_clip(page, current_bbox, budget)
            .with_context(|| format!("rendering clip for verify attempt {attempt}"))?;

        let image_path = work_dir.join(format!("attempt-{attempt:02}.png"));
        img.save(&image_path)
            .with_context(|| format!("saving verify crop {image_path:?}"))?;
        let output_path = work_dir.join(format!("attempt-{attempt:02}-result.json"));

        let prompt = build_prompt(kind, current_bbox, attempt, max_attempts);

        match run_codex_verify(&image_path, &schema_path, &output_path, work_dir, &prompt) {
            Ok(result) => {
                // Record the RAW suggestion (before capping/clamping) so the
                // history reflects what Codex actually said, not what we
                // did with it - `None` when it passed (schema says all four
                // adjustment values are 0 in that case, but semantically
                // "no adjustment was needed" reads better than `Some([0;4])`).
                let bbox_adjustment_pt = if result.passed {
                    None
                } else {
                    Some([
                        result.bbox_adjustment_pt.top,
                        result.bbox_adjustment_pt.bottom,
                        result.bbox_adjustment_pt.left,
                        result.bbox_adjustment_pt.right,
                    ])
                };
                history.push(VerificationAttempt {
                    attempt,
                    passed: result.passed,
                    issue: result.issue.clone(),
                    reason: result.reason.clone(),
                    bbox_adjustment_pt,
                });

                if result.passed {
                    return Ok((img, current_bbox, finish_outcome(history, true)));
                }

                if attempt == max_attempts {
                    return Ok((img, current_bbox, finish_outcome(history, false)));
                }

                current_bbox =
                    apply_adjustment(current_bbox, &result.bbox_adjustment_pt, geometry.width_pt, geometry.height_pt);
            }
            Err(e) => {
                // Soft failure: Codex itself couldn't be run or its output
                // couldn't be parsed. Consume this attempt; retry with the
                // same bbox if attempts remain, else fall back to returning
                // the current (unverified) crop.
                let msg = format!("verification_error: {e:#}");
                history.push(VerificationAttempt {
                    attempt,
                    passed: false,
                    issue: msg,
                    reason: String::new(),
                    bbox_adjustment_pt: None,
                });
                if attempt == max_attempts {
                    return Ok((img, current_bbox, finish_outcome(history, false)));
                }
                // else: loop again at the same bbox.
            }
        }
    }

    unreachable!("loop always returns by the time attempt == max_attempts");
}

fn build_prompt(kind: &str, bbox: BBoxPt, attempt: u32, max_attempts: u32) -> String {
    format!(
        "You are verifying a cropped image (attempt {attempt} of {max_attempts}) extracted \
from an academic paper PDF. The object's kind is \"{kind}\" (figure/table/formula/algorithm/\
aside). This crop is meant to be a clean, complete, standalone image of that object: fully \
visible on every side, not cut off, and not including excessive irrelevant surrounding content \
(paragraph text, unrelated neighboring figures, page furniture) beyond a small margin.\n\n\
The crop's current bounding box in PDF point space (72 points per inch, origin bottom-left of \
the page, y-up) is x0={x0:.1}, y0={y0:.1}, x1={x1:.1}, y1={y1:.1} - i.e. it currently spans \
{width:.1} points wide by {height:.1} points tall. Use this as the scale reference for any \
adjustment you suggest: e.g. on a {width:.1}pt-wide crop, an adjustment of {tenpct:.0} points is \
about 10% of the crop's width.\n\n\
Judge completeness/correctness and respond strictly per the provided JSON schema. Sign \
convention for bbox_adjustment_pt (all four values in PDF points, same unit as the bounding box \
above): a POSITIVE value on a side means EXPAND the crop outward on that side (the crop is \
currently cutting off content there, reveal more); a NEGATIVE value means SHRINK the crop inward \
on that side (remove extra irrelevant content there). top/bottom/left/right refer to the VISUAL \
orientation of this rendered image as you see it (top = upper edge of the image, not raw PDF \
y-coordinates). If passed=true, set all four bbox_adjustment_pt values to 0 and issue to \"none\".",
        attempt = attempt,
        max_attempts = max_attempts,
        kind = kind,
        x0 = bbox.x0,
        y0 = bbox.y0,
        x1 = bbox.x1,
        y1 = bbox.y1,
        width = bbox.width(),
        height = bbox.height(),
        tenpct = bbox.width() * 0.1,
    )
}

/// Applies a Codex-suggested adjustment to `bbox`, capping each side's
/// magnitude at `MAX_ADJUSTMENT_PT`, clamping the result to stay within the
/// page, and enforcing `MIN_DIM_PT` as a floor on both dimensions so a
/// shrink adjustment can never collapse or invert the box.
///
/// See the module doc comment for the sign convention this implements:
/// visual top/bottom map to the box's `y1`/`y0` respectively (PDF point
/// space is y-up), and visual left/right map directly to `x0`/`x1`.
fn apply_adjustment(bbox: BBoxPt, adj: &BboxAdjustment, page_width_pt: f32, page_height_pt: f32) -> BBoxPt {
    let cap = MAX_ADJUSTMENT_PT;
    let left = adj.left.clamp(-cap, cap);
    let right = adj.right.clamp(-cap, cap);
    let top = adj.top.clamp(-cap, cap);
    let bottom = adj.bottom.clamp(-cap, cap);

    let mut x0 = (bbox.x0 - left).max(0.0);
    let mut x1 = (bbox.x1 + right).min(page_width_pt);
    let mut y0 = (bbox.y0 - bottom).max(0.0);
    let mut y1 = (bbox.y1 + top).min(page_height_pt);

    if x1 - x0 < MIN_DIM_PT {
        let cx = (x0 + x1) / 2.0;
        x0 = (cx - MIN_DIM_PT / 2.0).max(0.0);
        x1 = (x0 + MIN_DIM_PT).min(page_width_pt);
    }
    if y1 - y0 < MIN_DIM_PT {
        let cy = (y0 + y1) / 2.0;
        y0 = (cy - MIN_DIM_PT / 2.0).max(0.0);
        y1 = (y0 + MIN_DIM_PT).min(page_height_pt);
    }

    BBoxPt { x0, y0, x1, y1 }
}

/// Shells out to `codex exec` with the exact invocation shape validated
/// manually against this machine's Codex CLI install (see README): image
/// input via `-i`, structured output via `--output-schema` + `-o`,
/// read-only sandbox (this is a judgment task, not code editing),
/// `--ephemeral` so we don't accumulate session files from one-shot calls,
/// and `--skip-git-repo-check` since `work_dir` is just a temp folder.
fn run_codex_verify(
    image_path: &Path,
    schema_path: &Path,
    output_path: &Path,
    work_dir: &Path,
    prompt: &str,
) -> Result<VerifyResult> {
    let mut cmd = Command::new("codex");
    cmd.arg("exec")
        .arg("-i")
        .arg(image_path)
        .arg("--output-schema")
        .arg(schema_path)
        .arg("-o")
        .arg(output_path)
        .arg("--sandbox")
        .arg("read-only")
        .arg("--ephemeral")
        .arg("--skip-git-repo-check")
        .arg("-C")
        .arg(work_dir)
        .arg(prompt);

    let output = run_with_timeout(cmd, Duration::from_secs(CODEX_TIMEOUT_SECS), false)
        .map_err(|e| anyhow::anyhow!("running codex exec: {e}"))?;

    if !output.status.success() {
        anyhow::bail!(
            "codex exec exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let raw = std::fs::read_to_string(output_path)
        .with_context(|| format!("reading codex output file {output_path:?}"))?;
    let result: VerifyResult = serde_json::from_str(&raw)
        .with_context(|| format!("parsing codex output JSON from {output_path:?}: {raw}"))?;
    Ok(result)
}

/// Spawns `cmd` and waits for it with a hard wall-clock cap, killing it if
/// exceeded. stderr is always drained on a background thread so a large
/// stderr buffer can never deadlock the poll loop against a full pipe.
/// stdout is drained the same way only when `capture_stdout` is set - the
/// main `codex exec` verification calls don't need it (the structured
/// result is read from the `-o` file instead, and the chatty transcript can
/// be large), but the quick `codex --version` preflight check does, to
/// surface the version string to the UI.
fn run_with_timeout(
    mut cmd: Command,
    timeout: Duration,
    capture_stdout: bool,
) -> Result<std::process::Output, String> {
    cmd.stdin(Stdio::null());
    cmd.stdout(if capture_stdout { Stdio::piped() } else { Stdio::null() });
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| e.to_string())?;

    let mut stdout_pipe = child.stdout.take();
    let stdout_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(pipe) = stdout_pipe.as_mut() {
            let _ = pipe.read_to_end(&mut buf);
        }
        buf
    });

    let mut stderr_pipe = child.stderr.take();
    let stderr_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(pipe) = stderr_pipe.as_mut() {
            let _ = pipe.read_to_end(&mut buf);
        }
        buf
    });

    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = stdout_handle.join();
                    let _ = stderr_handle.join();
                    return Err(format!("codex exec timed out after {}s", timeout.as_secs()));
                }
                std::thread::sleep(Duration::from_millis(150));
            }
            Err(e) => {
                let _ = stdout_handle.join();
                let _ = stderr_handle.join();
                return Err(e.to_string());
            }
        }
    };

    let stdout = stdout_handle.join().unwrap_or_default();
    let stderr = stderr_handle.join().unwrap_or_default();
    Ok(std::process::Output { status, stdout, stderr })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bbox(x0: f32, y0: f32, x1: f32, y1: f32) -> BBoxPt {
        BBoxPt { x0, y0, x1, y1 }
    }

    fn no_adjust() -> BboxAdjustment {
        BboxAdjustment { top: 0.0, bottom: 0.0, left: 0.0, right: 0.0 }
    }

    #[test]
    fn zero_adjustment_is_identity() {
        let b = bbox(100.0, 100.0, 200.0, 300.0);
        let out = apply_adjustment(b, &no_adjust(), 1000.0, 1000.0);
        assert!((out.x0 - b.x0).abs() < 1e-3);
        assert!((out.y0 - b.y0).abs() < 1e-3);
        assert!((out.x1 - b.x1).abs() < 1e-3);
        assert!((out.y1 - b.y1).abs() < 1e-3);
    }

    /// Positive "top" must EXPAND upward, which in this codebase's y-up
    /// `BBoxPt` means INCREASING y1 (mirrors `pdf::render`'s own
    /// "near-top box has high pdf y1" convention).
    #[test]
    fn positive_top_increases_y1() {
        let b = bbox(100.0, 100.0, 200.0, 300.0);
        let adj = BboxAdjustment { top: 20.0, bottom: 0.0, left: 0.0, right: 0.0 };
        let out = apply_adjustment(b, &adj, 1000.0, 1000.0);
        assert!((out.y1 - 320.0).abs() < 1e-3, "expected y1=320, got {}", out.y1);
        assert!((out.y0 - b.y0).abs() < 1e-3, "y0 should be untouched");
    }

    /// Positive "bottom" must EXPAND downward, i.e. DECREASING y0.
    #[test]
    fn positive_bottom_decreases_y0() {
        let b = bbox(100.0, 100.0, 200.0, 300.0);
        let adj = BboxAdjustment { top: 0.0, bottom: 20.0, left: 0.0, right: 0.0 };
        let out = apply_adjustment(b, &adj, 1000.0, 1000.0);
        assert!((out.y0 - 80.0).abs() < 1e-3, "expected y0=80, got {}", out.y0);
        assert!((out.y1 - b.y1).abs() < 1e-3, "y1 should be untouched");
    }

    /// Positive "left" must EXPAND leftward, i.e. DECREASING x0.
    #[test]
    fn positive_left_decreases_x0() {
        let b = bbox(100.0, 100.0, 200.0, 300.0);
        let adj = BboxAdjustment { top: 0.0, bottom: 0.0, left: 15.0, right: 0.0 };
        let out = apply_adjustment(b, &adj, 1000.0, 1000.0);
        assert!((out.x0 - 85.0).abs() < 1e-3, "expected x0=85, got {}", out.x0);
    }

    /// Positive "right" must EXPAND rightward, i.e. INCREASING x1.
    #[test]
    fn positive_right_increases_x1() {
        let b = bbox(100.0, 100.0, 200.0, 300.0);
        let adj = BboxAdjustment { top: 0.0, bottom: 0.0, left: 0.0, right: 15.0 };
        let out = apply_adjustment(b, &adj, 1000.0, 1000.0);
        assert!((out.x1 - 215.0).abs() < 1e-3, "expected x1=215, got {}", out.x1);
    }

    /// Negative values SHRINK inward: negative "top" must DECREASE y1,
    /// negative "left" must INCREASE x0.
    #[test]
    fn negative_values_shrink_inward() {
        let b = bbox(100.0, 100.0, 200.0, 300.0);
        let adj = BboxAdjustment { top: -20.0, bottom: -10.0, left: -5.0, right: -5.0 };
        let out = apply_adjustment(b, &adj, 1000.0, 1000.0);
        assert!((out.y1 - 280.0).abs() < 1e-3, "expected y1=280, got {}", out.y1);
        assert!((out.y0 - 110.0).abs() < 1e-3, "expected y0=110, got {}", out.y0);
        assert!((out.x0 - 105.0).abs() < 1e-3, "expected x0=105, got {}", out.x0);
        assert!((out.x1 - 195.0).abs() < 1e-3, "expected x1=195, got {}", out.x1);
    }

    #[test]
    fn adjustment_magnitude_is_capped() {
        let b = bbox(500.0, 500.0, 600.0, 600.0);
        let adj = BboxAdjustment { top: 10_000.0, bottom: 0.0, left: 0.0, right: 0.0 };
        let out = apply_adjustment(b, &adj, 5000.0, 5000.0);
        // Capped at MAX_ADJUSTMENT_PT (200), not the full 10_000 requested.
        assert!((out.y1 - 800.0).abs() < 1e-3, "expected y1 capped to 800, got {}", out.y1);
    }

    #[test]
    fn shrink_cannot_collapse_or_invert_box() {
        let b = bbox(100.0, 100.0, 110.0, 110.0); // 10x10pt box
        let adj = BboxAdjustment { top: -100.0, bottom: -100.0, left: -100.0, right: -100.0 };
        let out = apply_adjustment(b, &adj, 1000.0, 1000.0);
        assert!(out.width() >= MIN_DIM_PT - 1e-3, "width collapsed: {}", out.width());
        assert!(out.height() >= MIN_DIM_PT - 1e-3, "height collapsed: {}", out.height());
        assert!(out.x0 < out.x1);
        assert!(out.y0 < out.y1);
    }

    #[test]
    fn result_is_clamped_to_page_bounds() {
        let b = bbox(10.0, 10.0, 590.0, 780.0);
        let adj = BboxAdjustment { top: 50.0, bottom: 50.0, left: 50.0, right: 50.0 };
        let out = apply_adjustment(b, &adj, 600.0, 792.0);
        assert!(out.x0 >= 0.0);
        assert!(out.y0 >= 0.0);
        assert!(out.x1 <= 600.0, "x1 exceeded page width: {}", out.x1);
        assert!(out.y1 <= 792.0, "y1 exceeded page height: {}", out.y1);
    }
}
