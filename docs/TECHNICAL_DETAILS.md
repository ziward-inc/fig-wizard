# Technical details

## `manifest.json`

Each extraction run writes a `manifest.json` describing every exported object. Its `files`
field looks like:

```json
"files": { "format": "webp", "with_caption": "...", "no_caption": "..." }
```

Before association and export, every model-returned bounding box is expanded by 2% of its width on the left and right and 2% of its height on the top and bottom. The expanded box is clamped to the PDF page bounds and recorded in the manifest.

`format` is one of `"webp"`, `"avif"`, `"png"`, `"jpeg"`, `"jpegxl"` (lowercase, matching
the format picker's values — note `"jpegxl"` has no separator, while its file extension is
`.jxl`, mirroring how `"jpeg"` maps to a `.jpg` extension).

**Duplicate with/without-caption files when no caption is found.** Caption/number
association (`figure_title` for figures/tables/algorithms, `formula_number` for formulas)
is purely spatial (nearest box above/below or beside, same column, within a gap
threshold). When no caption/number box is associable, the object is still exported with
both a `_with-caption_` and a `_no-caption_` file — they're just byte-identical crops (the
with-caption render is skipped and the no-caption bitmap is reused). Every object always
has both files; `has_caption: false` in the manifest tells you when they're duplicates.

## JPEG XL encoding

JPEG XL is encoded in-process by the reference libjxl implementation (see
[CONTRIBUTING.md](../CONTRIBUTING.md) for the build-time dependency). The fixed
JPEG-style quality of 85 is converted with libjxl's public
`JxlEncoderDistanceFromQuality` mapping to a Butteraugli distance of 1.45. The encoder
emits a bare JPEG XL codestream beginning with `FF 0A`.

## Codex crop verification

When the "Verify crops with Codex" checkbox is on, before each detected object is exported
the app shells out to the local `codex` CLI (used purely as a multimodal judge — no code
is read or edited) with the current crop image, asking it to judge, per a structured JSON
schema, whether the crop is a clean, complete, standalone image of that object.

Finder-launched macOS apps don't inherit a terminal's `PATH`, so FigWizard resolves Codex
from both `PATH` and common per-user install locations (`~/.local/bin`,
`~/.npm-global/bin`). For npm installs, it launches the platform-native Codex binary
inside the package instead of the JS wrapper, avoiding the wrapper's
`#!/usr/bin/env node` `PATH` failure.

- If Codex says the crop passes, it's exported as-is.
- If Codex flags an issue (cut off on some side, or including too much irrelevant extra
  content), it also returns a suggested bounding-box correction in PDF points. For
  expansion-only feedback, the app adds a 2%-of-bbox-dimension safety margin per side;
  shrink-only or mixed feedback is applied as suggested. The result is capped, clamped,
  re-rendered, and re-verified — up to 3 total attempts per object (see `MAX_ATTEMPTS` in
  `src-tauri/src/verify/mod.rs`).
- One verification pass runs per object, against its own (no-caption) bounding box; the
  corrected box is reused for both the no-caption and with-caption crops (re-unioned with
  the caption box for the latter) — the with-caption variant isn't verified separately.
- If Codex itself fails (binary missing/unauthenticated, network hiccup, timeout,
  malformed output), that attempt is a soft failure — consumed like any other failed
  attempt, falling back to the last-rendered crop rather than crashing the job. Before
  starting a run with the checkbox on, the app runs `codex --version` upfront and refuses
  to start if it's unavailable.

**Manifest fields.** `manifest.json` gets a `verification: { enabled, attempts, passed,
last_issue, history }` field per object (absent when verification was off for that run).
The results list shows a badge per row: "✓ 1 try" (passed first try), "⟳ N tries" (passed
after corrections), or "⚠ N tries, still flagged" (never passed within budget).

`verification.history` has one entry per real attempt, in order:

```json
{ "attempt": 1, "passed": false, "issue": "extra_content_top", "reason": "...", "bbox_adjustment_pt": [12.0, 0.0, 0.0, 0.0] }
```

`bbox_adjustment_pt` is `[top, bottom, left, right]` in PDF points — Codex's raw
suggestion before capping/clamping; `null` on a passed attempt or a soft failure.
`attempts`/`passed`/`last_issue` summarize the same data (`attempts == history.length`,
`last_issue` mirrors the last entry's `issue`).

Codex only sees the rendered crop image plus the crop's current size in PDF points as a
scale reference — its correction is a visual judgment call, not a pixel-precise
measurement. Occasional over/under-correction, or still being flagged after 3 attempts, is
expected; `last_issue`/`last_reason` and the "still flagged" badge exist so you can spot
and manually review those cases.

## Performance notes

A 15-page paper with ~17 extracted objects takes on the order of ten-plus minutes on a
laptop CPU with AVIF selected (the slowest of the 5 formats to encode; PNG/JPEG/WebP are
faster). Per-page timing varies a lot with page content and with how much else is
competing for CPU — one 12-page paper was observed taking well over an hour on a heavily
CPU-contended machine, versus ~15 minutes on an otherwise-idle one. Treat both as data
points, not guarantees.

Codex crop verification adds a real network+time cost: each attempt is one `codex exec`
call (~8-90s depending on model/reasoning effort and load), up to 3 attempts per object
worst case. On a paper with dozens of objects this can add many minutes on top of the core
pipeline — why it defaults off. The cancel button is checked between attempts as well as
between objects.
