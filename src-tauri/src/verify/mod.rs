//! Optional, off-by-default runtime crop verification: shells out to
//! whichever multimodal CLI the caller selected as `VerifyBackend` (`codex`,
//! OpenAI's coding agent, or `claude`, Claude Code - both used here purely as
//! a multimodal judge, no code editing happens) with the rendered crop image
//! and asks it to judge whether the crop is a clean, complete, standalone
//! image of the detected object. If not, the backend proposes a
//! bounding-box correction (in PDF points, on the visual sides of the
//! image); we apply it, re-render, and re-verify, up to `MAX_ATTEMPTS` total
//! tries. The two backends are mutually exclusive per run - see
//! `VerifyBackend`.
//!
//! This module is purely additive: any failure to invoke/parse the backend
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
use std::env;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::pdf::render::{page_geometry, render_clip, ClipRenderBudget};
use crate::pipeline::types::{BBoxPt, VerificationAttempt, VerifyBackend};

/// Total attempts per object: 1 initial check + up to 2 corrective
/// re-renders. Named/exported so it's easy to find and tune.
pub const MAX_ATTEMPTS: u32 = 3;

/// Wall-clock cap on a single verification-backend invocation (`codex exec`
/// or `claude -p`). A manual timing run against a real crop (see dev notes /
/// README) took ~8s at default reasoning effort; this leaves generous
/// headroom for slower prompts without letting a hung/stalled call block the
/// pipeline indefinitely.
const VERIFY_TIMEOUT_SECS: u64 = 90;

/// Model alias passed to every `claude -p` verification call, pinning cost
/// regardless of whatever model the user's Claude Code install otherwise
/// defaults to (which could be a pricier tier) - this loop can make up to
/// `MAX_ATTEMPTS` calls per object across every detected object in a PDF, so
/// an unpinned default would make cost unpredictable. `"sonnet"` is a
/// standing alias Claude Code resolves to its latest Sonnet model, not a
/// version string that goes stale.
const CLAUDE_MODEL: &str = "sonnet";

/// Absolute cap (in PDF points) on how far a single corrective adjustment
/// may move any one side, guarding against a wild/degenerate Codex
/// suggestion blowing up the crop.
const MAX_ADJUSTMENT_PT: f32 = 200.0;

/// Fraction of the current bbox dimension added to every side when Codex
/// only asks to expand (or returns a failed verdict with a zero adjustment).
/// Any adjustment that includes a shrink is applied exactly as suggested.
const EXPANSION_MARGIN_RATIO: f32 = 0.02;

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

/// Resolves `name` from the process `PATH` and the common per-user install
/// locations that Finder-launched macOS apps do not inherit (both the
/// `codex` and `claude` CLIs can land in any of these depending on how the
/// user installed them).
fn resolve_binary(name: &str) -> Result<PathBuf, String> {
    let mut candidates = Vec::new();

    if let Some(path) = env::var_os("PATH") {
        candidates.extend(env::split_paths(&path).map(|dir| dir.join(name)));
    }

    if let Some(home) = env::var_os("HOME").map(PathBuf::from) {
        for relative in [
            ".local/bin",
            ".npm-global/bin",
            ".npm/bin",
            ".cargo/bin",
            ".bun/bin",
            ".volta/bin",
            "Library/pnpm",
        ] {
            candidates.push(home.join(relative).join(name));
        }
    }

    candidates.push(PathBuf::from("/opt/homebrew/bin").join(name));
    candidates.push(PathBuf::from("/usr/local/bin").join(name));

    candidates
        .into_iter()
        .find(|candidate| is_executable(candidate))
        .ok_or_else(|| format!("{name} binary not found on PATH or in common user install locations"))
}

/// npm installs of Codex use a JavaScript launcher whose `#!/usr/bin/env
/// node` would have the same GUI `PATH` problem, so those launchers are
/// mapped to the platform-native Codex binary shipped inside the npm
/// package. This redirect is Codex/npm-package-specific and doesn't apply to
/// `claude`.
fn resolve_codex_binary() -> Result<PathBuf, String> {
    let candidate = resolve_binary("codex")?;
    Ok(resolve_npm_native_binary(&candidate).unwrap_or(candidate))
}

/// Resolves the `claude` (Claude Code) CLI binary the same way `codex` is
/// resolved - no npm-native-binary redirect needed since Claude Code ships a
/// native binary directly.
fn resolve_claude_binary() -> Result<PathBuf, String> {
    resolve_binary("claude")
}

fn is_executable(path: &Path) -> bool {
    path.metadata()
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

fn resolve_npm_native_binary(launcher: &Path) -> Option<PathBuf> {
    let launcher = launcher.canonicalize().ok()?;
    if launcher.file_name()?.to_str()? != "codex.js" {
        return None;
    }

    let package_root = launcher.parent()?.parent()?;
    let (platform_package, target_triple) = match (env::consts::OS, env::consts::ARCH) {
        ("macos", "aarch64") => ("codex-darwin-arm64", "aarch64-apple-darwin"),
        ("macos", "x86_64") => ("codex-darwin-x64", "x86_64-apple-darwin"),
        ("linux", "aarch64") => ("codex-linux-arm64", "aarch64-unknown-linux-musl"),
        ("linux", "x86_64") => ("codex-linux-x64", "x86_64-unknown-linux-musl"),
        _ => return None,
    };
    let package = Path::new("@openai").join(platform_package);
    let binary_tail = Path::new("vendor").join(target_triple).join("bin").join("codex");

    let bundled = package_root.join(&binary_tail);
    if is_executable(&bundled) {
        return Some(bundled);
    }

    for ancestor in package_root.ancestors().take(8) {
        let candidate = ancestor.join("node_modules").join(&package).join(&binary_tail);
        if is_executable(&candidate) {
            return Some(candidate);
        }
    }

    None
}

/// Quick check for whether the resolved `codex` binary executes without
/// erroring. Used as an upfront preflight before starting a whole extraction
/// run with verification enabled.
pub fn codex_available() -> Result<String, String> {
    let binary = resolve_codex_binary()?;
    let mut cmd = Command::new(&binary);
    cmd.arg("--version");
    match run_with_timeout(cmd, Duration::from_secs(10), true) {
        Ok(output) if output.status.success() => Ok(format!(
            "{} ({})",
            String::from_utf8_lossy(&output.stdout).trim(),
            binary.display()
        )),
        Ok(output) => Err(format!(
            "codex --version exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )),
        Err(e) => Err(format!("codex binary not runnable at {}: {e}", binary.display())),
    }
}

/// Quick check for whether the resolved `claude` binary executes without
/// erroring. Used as an upfront preflight before starting a whole extraction
/// run with verification enabled.
pub fn claude_available() -> Result<String, String> {
    let binary = resolve_claude_binary()?;
    let mut cmd = Command::new(&binary);
    cmd.arg("--version");
    match run_with_timeout(cmd, Duration::from_secs(10), true) {
        Ok(output) if output.status.success() => Ok(format!(
            "{} ({})",
            String::from_utf8_lossy(&output.stdout).trim(),
            binary.display()
        )),
        Ok(output) => Err(format!(
            "claude --version exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )),
        Err(e) => Err(format!("claude binary not runnable at {}: {e}", binary.display())),
    }
}

/// Renders `initial_bbox_pt`, asks the selected `backend` to verify it, and
/// if it fails, applies the backend's suggested correction and retries - up
/// to `max_attempts` total tries. Always returns `Ok` unless the underlying
/// PDF render itself fails (which would fail the export anyway); any
/// backend/process/parse failure is captured as a soft failure inside
/// `VerifyOutcome` instead. `backend` must not be `VerifyBackend::Off` -
/// callers only invoke this function when verification is enabled.
///
/// `cancel` is polled between attempts (in addition to the between-object
/// check already done by the caller) so a multi-attempt verification loop
/// doesn't make Cancel unresponsive.
#[allow(clippy::too_many_arguments)]
pub fn verify_and_correct_crop(
    page: &PdfPage,
    kind: &str,
    initial_bbox_pt: BBoxPt,
    budget: ClipRenderBudget,
    max_attempts: u32,
    work_dir: &Path,
    cancel: &AtomicBool,
    backend: VerifyBackend,
) -> Result<(RgbImage, BBoxPt, VerifyOutcome)> {
    std::fs::create_dir_all(work_dir)
        .with_context(|| format!("creating verify work dir {work_dir:?}"))?;

    // Only the Codex path needs the schema on disk (passed via
    // `--output-schema`); Claude gets the same schema inline via
    // `--json-schema`.
    let schema_path = work_dir.join("verify_schema.json");
    if backend == VerifyBackend::Codex {
        std::fs::write(&schema_path, VERIFY_SCHEMA_JSON)
            .with_context(|| format!("writing {schema_path:?}"))?;
    }

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

        let result = match backend {
            VerifyBackend::Codex => run_codex_verify(&image_path, &schema_path, &output_path, work_dir, &prompt),
            VerifyBackend::Claude => run_claude_verify(&image_path, work_dir, &prompt),
            VerifyBackend::Off => {
                Err(anyhow::anyhow!("verify_and_correct_crop called with backend=Off"))
            }
        };

        match result {
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

/// Applies a Codex-suggested adjustment to `bbox`. Expansion-only feedback
/// gets `EXPANSION_MARGIN_RATIO` of the current width added on the left and
/// right, and the same ratio of the current height on the top and bottom.
/// Shrink-only and mixed expand/shrink feedback are applied without that
/// margin. Each side is then capped at `MAX_ADJUSTMENT_PT`, the result is
/// clamped to the page, and `MIN_DIM_PT` is enforced as a floor on both
/// dimensions so a shrink adjustment can never collapse or invert the box.
///
/// See the module doc comment for the sign convention this implements:
/// visual top/bottom map to the box's `y1`/`y0` respectively (PDF point
/// space is y-up), and visual left/right map directly to `x0`/`x1`.
fn apply_adjustment(bbox: BBoxPt, adj: &BboxAdjustment, page_width_pt: f32, page_height_pt: f32) -> BBoxPt {
    let cap = MAX_ADJUSTMENT_PT;
    let (horizontal_margin, vertical_margin) =
        if adj.top >= 0.0 && adj.bottom >= 0.0 && adj.left >= 0.0 && adj.right >= 0.0 {
            (bbox.width() * EXPANSION_MARGIN_RATIO, bbox.height() * EXPANSION_MARGIN_RATIO)
        } else {
            (0.0, 0.0)
        };
    let left = (adj.left + horizontal_margin).clamp(-cap, cap);
    let right = (adj.right + horizontal_margin).clamp(-cap, cap);
    let top = (adj.top + vertical_margin).clamp(-cap, cap);
    let bottom = (adj.bottom + vertical_margin).clamp(-cap, cap);

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
    let binary = resolve_codex_binary().map_err(anyhow::Error::msg)?;
    let mut cmd = Command::new(binary);
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

    let output = run_with_timeout(cmd, Duration::from_secs(VERIFY_TIMEOUT_SECS), false)
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

/// Shells out to `claude -p` (Claude Code's non-interactive print mode) as
/// the alternative multimodal judge to Codex. Unlike Codex (which takes the
/// image directly via `-i`), Claude Code has no direct image-attach flag, so
/// the prompt explicitly instructs the model to open `image_path` with its
/// `Read` tool first - `--tools "Read"` is the only tool granted, and
/// `--permission-mode bypassPermissions` skips the interactive confirmation
/// that would otherwise block a headless call. Structured output is
/// enforced via `--json-schema` (Claude Code's equivalent of Codex's
/// `--output-schema`) and read back from stdout (`--output-format json`)
/// rather than an output file. `--disable-slash-commands`,
/// `--setting-sources ""`, and `--strict-mcp-config` strip the user's
/// skills/plugins/MCP servers from the loaded system prompt - both to avoid
/// unrelated tools/services adding latency or failure surface to a headless
/// verification call, and because loading them roughly 7x'd the measured
/// per-call token cost in manual testing. `--no-session-persistence` avoids
/// littering `~/.claude` with one-shot verification sessions. `--model` is
/// pinned to `CLAUDE_MODEL` (see its doc comment) for cost predictability.
fn run_claude_verify(image_path: &Path, work_dir: &Path, prompt: &str) -> Result<VerifyResult> {
    let binary = resolve_claude_binary().map_err(anyhow::Error::msg)?;
    let full_prompt = format!(
        "Use the Read tool to open the image file at {} before answering. {prompt}",
        image_path.display(),
    );

    let mut cmd = Command::new(binary);
    cmd.current_dir(work_dir)
        .arg("-p")
        .arg(&full_prompt)
        .arg("--output-format")
        .arg("json")
        .arg("--json-schema")
        .arg(VERIFY_SCHEMA_JSON)
        .arg("--permission-mode")
        .arg("bypassPermissions")
        .arg("--tools")
        .arg("Read")
        .arg("--add-dir")
        .arg(work_dir)
        .arg("--disable-slash-commands")
        .arg("--setting-sources")
        .arg("")
        .arg("--strict-mcp-config")
        .arg("--no-session-persistence")
        .arg("--model")
        .arg(CLAUDE_MODEL);

    let output = run_with_timeout(cmd, Duration::from_secs(VERIFY_TIMEOUT_SECS), true)
        .map_err(|e| anyhow::anyhow!("running claude -p: {e}"))?;

    if !output.status.success() {
        anyhow::bail!(
            "claude -p exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    parse_claude_output(&String::from_utf8_lossy(&output.stdout))
}

/// Parses `claude -p --output-format json` stdout into a `VerifyResult`.
/// The exact top-level shape has been observed to vary (a single result
/// object in some configurations, a JSON array of the full transcript with
/// the result event last in others) depending on which flags are set, so
/// this handles both rather than assuming one: it finds the object with
/// `"type": "result"` (itself, or the last matching array element), bails
/// with that event's own error text if `is_error` is set, then reads the
/// already-parsed `structured_output` field (rather than re-parsing the
/// `result` string field, which is redundant and only present as a
/// convenience for text consumers).
fn parse_claude_output(raw: &str) -> Result<VerifyResult> {
    let value: serde_json::Value = serde_json::from_str(raw.trim())
        .with_context(|| format!("parsing claude -p JSON output: {raw}"))?;

    let result_event = match &value {
        serde_json::Value::Array(events) => events
            .iter()
            .rev()
            .find(|e| e.get("type").and_then(|t| t.as_str()) == Some("result"))
            .ok_or_else(|| anyhow::anyhow!("no \"result\" event in claude -p output: {raw}"))?,
        serde_json::Value::Object(_) => &value,
        _ => anyhow::bail!("unexpected claude -p JSON shape: {raw}"),
    };

    if result_event.get("is_error").and_then(|v| v.as_bool()) == Some(true) {
        let msg = result_event.get("result").and_then(|v| v.as_str()).unwrap_or("unknown error");
        anyhow::bail!("claude -p returned an error: {msg}");
    }

    let structured = result_event
        .get("structured_output")
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("claude -p result missing structured_output: {raw}"))?;

    serde_json::from_value(structured)
        .with_context(|| format!("parsing claude structured_output into VerifyResult: {raw}"))
}

/// Spawns `cmd` and waits for it with a hard wall-clock cap, killing it if
/// exceeded. stderr is always drained on a background thread so a large
/// stderr buffer can never deadlock the poll loop against a full pipe.
/// stdout is drained the same way only when `capture_stdout` is set - the
/// main `codex exec` verification call doesn't need it (the structured
/// result is read from the `-o` file instead, and the chatty transcript can
/// be large), but the `claude -p` verification call does (its structured
/// result comes back on stdout), as does the quick `--version` preflight
/// check for either backend, to surface the version string to the UI.
///
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
                    return Err(format!("process timed out after {}s", timeout.as_secs()));
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
    #[ignore = "requires an installed Codex CLI"]
    fn installed_codex_resolves_to_runnable_binary() {
        let binary = resolve_codex_binary().expect("resolve installed Codex CLI");
        assert!(is_executable(&binary), "not executable: {}", binary.display());
        assert_ne!(binary.file_name().and_then(|name| name.to_str()), Some("codex.js"));
        let status = codex_available().expect("run installed Codex CLI");
        assert!(status.contains("codex-cli"), "unexpected status: {status}");
    }

    #[test]
    #[ignore = "requires an installed Claude Code CLI"]
    fn installed_claude_resolves_to_runnable_binary() {
        let binary = resolve_claude_binary().expect("resolve installed Claude Code CLI");
        assert!(is_executable(&binary), "not executable: {}", binary.display());
        let status = claude_available().expect("run installed Claude Code CLI");
        assert!(status.contains("Claude Code"), "unexpected status: {status}");
    }

    #[test]
    fn zero_non_shrink_adjustment_expands_every_side_by_ratio() {
        let b = bbox(100.0, 100.0, 200.0, 300.0);
        let out = apply_adjustment(b, &no_adjust(), 1000.0, 1000.0);
        assert!((out.x0 - 98.0).abs() < 1e-3);
        assert!((out.y0 - 96.0).abs() < 1e-3);
        assert!((out.x1 - 202.0).abs() < 1e-3);
        assert!((out.y1 - 304.0).abs() < 1e-3);
    }

    /// Positive "top" must EXPAND upward, which in this codebase's y-up
    /// `BBoxPt` means INCREASING y1 (mirrors `pdf::render`'s own
    /// "near-top box has high pdf y1" convention).
    #[test]
    fn positive_top_increases_y1() {
        let b = bbox(100.0, 100.0, 200.0, 300.0);
        let adj = BboxAdjustment { top: 20.0, bottom: 0.0, left: 0.0, right: 0.0 };
        let out = apply_adjustment(b, &adj, 1000.0, 1000.0);
        assert!((out.y1 - 324.0).abs() < 1e-3, "expected y1=324, got {}", out.y1);
        assert!((out.y0 - 96.0).abs() < 1e-3, "expected y0=96, got {}", out.y0);
        assert!((out.x0 - 98.0).abs() < 1e-3, "expected x0=98, got {}", out.x0);
        assert!((out.x1 - 202.0).abs() < 1e-3, "expected x1=202, got {}", out.x1);
    }

    /// Positive "bottom" must EXPAND downward, i.e. DECREASING y0.
    #[test]
    fn positive_bottom_decreases_y0() {
        let b = bbox(100.0, 100.0, 200.0, 300.0);
        let adj = BboxAdjustment { top: 0.0, bottom: 20.0, left: 0.0, right: 0.0 };
        let out = apply_adjustment(b, &adj, 1000.0, 1000.0);
        assert!((out.y0 - 76.0).abs() < 1e-3, "expected y0=76, got {}", out.y0);
        assert!((out.y1 - 304.0).abs() < 1e-3, "expected y1=304, got {}", out.y1);
    }

    /// Positive "left" must EXPAND leftward, i.e. DECREASING x0.
    #[test]
    fn positive_left_decreases_x0() {
        let b = bbox(100.0, 100.0, 200.0, 300.0);
        let adj = BboxAdjustment { top: 0.0, bottom: 0.0, left: 15.0, right: 0.0 };
        let out = apply_adjustment(b, &adj, 1000.0, 1000.0);
        assert!((out.x0 - 83.0).abs() < 1e-3, "expected x0=83, got {}", out.x0);
        assert!((out.x1 - 202.0).abs() < 1e-3, "expected x1=202, got {}", out.x1);
    }

    /// Positive "right" must EXPAND rightward, i.e. INCREASING x1.
    #[test]
    fn positive_right_increases_x1() {
        let b = bbox(100.0, 100.0, 200.0, 300.0);
        let adj = BboxAdjustment { top: 0.0, bottom: 0.0, left: 0.0, right: 15.0 };
        let out = apply_adjustment(b, &adj, 1000.0, 1000.0);
        assert!((out.x1 - 217.0).abs() < 1e-3, "expected x1=217, got {}", out.x1);
        assert!((out.x0 - 98.0).abs() < 1e-3, "expected x0=98, got {}", out.x0);
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
    fn single_side_shrink_does_not_expand_other_sides() {
        let b = bbox(100.0, 100.0, 200.0, 300.0);
        let adj = BboxAdjustment { top: -20.0, bottom: 0.0, left: 0.0, right: 0.0 };
        let out = apply_adjustment(b, &adj, 1000.0, 1000.0);
        assert!((out.y1 - 280.0).abs() < 1e-3, "expected y1=280, got {}", out.y1);
        assert!((out.y0 - b.y0).abs() < 1e-3, "y0 should be untouched");
        assert!((out.x0 - b.x0).abs() < 1e-3, "x0 should be untouched");
        assert!((out.x1 - b.x1).abs() < 1e-3, "x1 should be untouched");
    }

    #[test]
    fn mixed_expand_and_shrink_feedback_is_applied_exactly() {
        let b = bbox(100.0, 100.0, 200.0, 300.0);
        let adj = BboxAdjustment { top: 20.0, bottom: -10.0, left: 15.0, right: -5.0 };
        let out = apply_adjustment(b, &adj, 1000.0, 1000.0);
        assert!((out.y1 - 320.0).abs() < 1e-3, "expected y1=320, got {}", out.y1);
        assert!((out.y0 - 110.0).abs() < 1e-3, "expected y0=110, got {}", out.y0);
        assert!((out.x0 - 85.0).abs() < 1e-3, "expected x0=85, got {}", out.x0);
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
