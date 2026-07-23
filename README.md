# FigWizard

A macOS-only Tauri app that extracts figures, tables, formulas, and algorithm/pseudocode
blocks from academic paper PDFs, exporting each as a near-4K crop in **one** user-selected
image format (WebP, AVIF, PNG, JPEG, or JPEG XL - quality 85 for the lossy ones), with and
without an associated caption.

## Running it

```sh
pnpm install
pnpm tauri dev
```

### Frontend stack

The UI (`src`) is a Vite + React + TypeScript app using [shadcn/ui](https://ui.shadcn.com)
components (on top of `@base-ui/react` primitives, not Radix - per this project's
`components.json`, from the `beXRTXBi` preset) and Tailwind v4. It talks to the Rust side via
`@tauri-apps/api` (`core`/`event`/`webview`) rather than the `window.__TAURI__` global -
`tauri.conf.json` sets `app.withGlobalTauri: false` accordingly. The UI font is
[SUITE Variable](https://github.com/sun-typeface/SUITE), vendored locally at
`public/fonts/SUITE-Variable.woff2` (not loaded from a CDN, so the app doesn't need
network access to render its own chrome) and wired in as `--font-sans` in `src/index.css`;
Geist Mono remains `--font-mono` for the monospace bits (file paths, etc).

Layout of `src`:

- `App.tsx` - top-level state (current PDF, output dir/format, job/progress state, results manifest) and the Tauri event-listener wiring; composes the section components below into the four numbered cards plus the results list.
- `components/app/` - the app's own feature components (`ModelBanner`, `PdfDropZone`, `FormatPicker`, `VerifySection`, `ExtractSection`, `ResultsGallery`, and `VerificationBadge`). Each corresponds 1:1 to a section/behavior of the original vanilla-JS UI.
- `components/ui/` - shadcn-generated primitives (button, card, dialog, etc.) - treat
  these as generated code; re-run `pnpm dlx shadcn@latest add <name>` from the repository root to
  add more or `--overwrite` to regenerate rather than hand-editing them.
- `lib/tauri-types.ts` - TypeScript mirrors of the Rust types in `src-tauri/src/commands.rs`
  / `src-tauri/src/pipeline/types.rs`. Note the casing split: pipeline events
  (`page-detected`, `object-exported`, etc.) are hand-built with `serde_json::json!` and
  use **camelCase** keys (`jobId`, `pageIndex`, ...), while `manifest.json` / command
  return values use serde's default **snake_case** (`page_index`, `has_caption`, ...).
  This is intentional on the Rust side, not a typo - keep it in mind when adding fields.
- `lib/tauri-commands.ts` - thin typed wrappers around `invoke("command_name", ...)` for
  every `#[tauri::command]` in `commands.rs`.
- `lib/format.ts` - small pure helpers (`pdfStem`, `dirName`, `formatBytes`).

Because `App.tsx` sets up the `listen()` calls once on mount, but needs the *current*
`currentJobId`/`currentPdf`/`currentOutputDir` inside those long-lived callbacks (to filter
events by job id, or to call `list_results` after `extraction-complete`), those three
pieces of state are mirrored into refs (`currentJobIdRef` etc.) that are updated in lockstep
with the corresponding `useState` setters, rather than read directly from the closed-over
state.

To add more shadcn components: `pnpm dlx shadcn@latest add <name>` from the repository root.

On first run, if the detection model / PDFium library aren't present yet, the app shows a
"Download model (~125MB)" banner. Clicking it downloads:

- The PP-DocLayoutV3 ONNX model + its `config.json` (label list) from
  `huggingface.co/alex-dinh/PP-DocLayoutV3-ONNX`.
- The macOS arm64 PDFium dylib from a `bblanchon/pdfium-binaries` GitHub release
  (`pdfium-mac-arm64.tgz`, only `lib/libpdfium.dylib` is kept).

Both land under the OS-managed app-data directory (not inside the app bundle), so a
packaged release doesn't need to ship ~130MB of model weights. During local development
(`cargo tauri dev` / debug builds only), the app also falls back to the gitignored copies
already checked out at `src-tauri/models/` and `src-tauri/binaries/pdfium/` if present, so
contributors who already have those don't need to re-download anything. That fallback path
is compiled out of release builds.

## Installing via the one-line script

`install.sh` (at the repo root) looks up the latest GitHub release, downloads its `.dmg`, installs `FigWizard.app` into `/Applications`, and launches it - no `git clone`, Xcode, or Rust toolchain needed:

If the installed app is already the latest version, the installer asks before downloading and overwriting it. Enter `y` or `yes` to reinstall; pressing Enter or answering `n` leaves the existing installation unchanged. A non-interactive run stops safely because it cannot obtain overwrite confirmation.

```sh
curl -fsSL https://raw.githubusercontent.com/ziward-inc/fig-wizard/main/install.sh | bash
```

```sh
wget -qO- https://raw.githubusercontent.com/ziward-inc/fig-wizard/main/install.sh | bash
```

It requires only `curl` or `wget`, plus `jq` (falls back to `python3`, which ships with
macOS, if `jq` isn't installed) to parse the GitHub API response. Like the `.dmg` release
itself, the installed app is ad-hoc signed only, not notarized - see "Notarization status"
below.

## Installing via `cargo install`

As an alternative to the `.dmg` release, the one-line script above, or running from source
with `pnpm tauri dev`, the app can be installed as a plain binary via Cargo (confirmed
working: `cargo install --path src-tauri --locked` successfully builds and installs a
`figwizard` binary):

```sh
git clone https://github.com/ziward-inc/fig-wizard.git
cd fig-wizard
cargo install --path src-tauri --locked
```

This puts a `figwizard` executable on your `$PATH` (typically `~/.cargo/bin/figwizard`),
which launches the same GUI app when run from a terminal.

**This is not the same as a proper macOS `.app`, though** - `cargo install` only produces
a bare Mach-O executable, not an app bundle:
- No Dock icon, no `Info.plist`, no Spotlight/Launchpad visibility as an installed app -
  it only runs when launched from a terminal (or wrapped in your own launcher).
- The custom app icon (`icons/icon.icns` etc.) is baked into the `.app`/`.dmg` bundle by
  Tauri's *bundler* step (`tauri build`), which `cargo install` doesn't invoke - you get
  Rust's default binary, no icon customization.
- Still requires the same first-run model/PDFium download described above (unaffected by
  the install method).

**Not published to crates.io, and not planned.** `cargo install figwizard` (installing by
crate name from the registry, without `--path`/a local checkout) doesn't work and isn't
offered: `cargo publish --dry-run` fails outright, because `tauri.conf.json`'s
`frontendDist` points to `"../dist"` (the Vite build output) - outside the `src-tauri/`
crate root - and a published
crate package only contains files within its own directory, so the frontend simply
wouldn't be there for anyone installing from the registry. Fixing this would require
restructuring the frontend to live inside `src-tauri/`. Separately, crates.io is a fully
public registry with no private-crate option and no real way to free a claimed name later,
which cuts against this repo's private/personal-org distribution scope - so even setting
the technical issue aside, this was a deliberate choice, not just an oversight. The `.dmg`
release and the `--path`-based install above remain the supported ways to get the app.

Use the `.dmg` release for a normal double-click-to-install experience; use `cargo
install` if you specifically want a CLI-launchable binary (e.g. for scripting, or to avoid
Gatekeeper's `.app` quarantine flow entirely - see notarization below).

## Notarization status

The `.dmg`/`.app` published in [Releases](https://github.com/ziward-inc/fig-wizard/releases)
is **ad-hoc signed only** - confirmed via `codesign -dv FigWizard.app`, which reports
`flags=0x20002(adhoc,linker-signed)` and `TeamIdentifier=not set`. There is no Apple
Developer ID signature and the app is **not notarized**. Concretely, this means:

- macOS Gatekeeper will refuse to open it via a normal double-click ("cannot be opened
  because it is from an unidentified developer" / "Apple could not verify...").
- To run it anyway: right-click the `.app` → **Open** → confirm in the dialog (only needed
  once), or run `xattr -cr /Applications/FigWizard.app` to strip the quarantine attribute.
- The `cargo install` route above sidesteps this entirely, since Gatekeeper's quarantine
  flow applies to app bundles downloaded via a browser/Finder, not to binaries built and
  installed locally by Cargo.

### What real notarization would require (not currently set up)

To ship a build that opens with no warnings, you'd need:
1. A paid **Apple Developer Program** membership (Team ID).
2. A **Developer ID Application** signing certificate installed in the build machine's
   keychain.
3. Tauri's bundler configured to sign during `tauri build` - as of Tauri v2 this is
   generally driven by environment variables at build time such as
   `APPLE_SIGNING_IDENTITY`, `APPLE_CERTIFICATE`, and `APPLE_CERTIFICATE_PASSWORD` for
   code signing, plus `APPLE_ID`/`APPLE_PASSWORD` (or an App Store Connect API key +
   `APPLE_TEAM_ID`) for the actual notarization submission to Apple.
   **These exact variable names weren't independently verified against Tauri's source in
   this environment - check the current [Tauri v2 macOS code-signing
   docs](https://v2.tauri.app/distribute/sign/macos/) before relying on them.**
4. `tauri build` submits the app to Apple's notary service automatically once signing is
   configured; add `--skip-stapling` for the very first notarization run (per `tauri build
   --help`, initial notarization "can take multiple hours" - this flag lets the build
   finish without blocking on it, at the cost of not stapling the ticket onto the app for
   offline Gatekeeper checks until a later run).

None of this is configured in this repo today - v0.1.0 ships ad-hoc signed, matching a
personal/small-org distribution scope rather than a public, warning-free release.

## Using it

1. Drag a PDF onto the drop zone (or click "Choose PDF…").
2. Pick an output folder (defaults to an `extracted/` folder next to the PDF; override with
   "Choose output folder…").
3. Choose an output format: **WebP, AVIF, PNG, JPEG, or JPEG XL** (radio buttons - exactly
   one is active per run). **WebP is the default.** JPEG XL is encoded by libjxl bundled into the app and requires no separate install.
4. Click "Extract" and watch the live per-page progress and running counts by kind.
5. When done, browse the text-only results list grouped by page. Click "OPEN" to open the result directory in Finder.

Extraction writes to
`<output_dir>/<pdf-stem>/page-NNNN/<kind>-NN_{with,no}-caption_q85.<ext>` for the lossy
formats (`webp`/`avif`/`jpg`/`jxl`) or `<kind>-NN_{with,no}-caption.png` (no quality suffix,
since PNG is lossless) for PNG, plus a `manifest.json` describing every exported object
(kind, page, bbox, score, caption association, file paths).

### Picking a different format per run

Every extraction run produces exactly **2 files per object** (with-caption and
no-caption) in the single format selected in step 3 above - not 4 files in two fixed
formats like earlier versions of this app. Re-running extraction on the same PDF with a
different format selected will (over)write a fresh set of files (and a fresh
`manifest.json`) using that format.

The results UI intentionally does not render the exported images inline. Use "OPEN" to inspect any format directly in Finder without loading every image into the WebView.

### `manifest.json` schema change: `files` shape

**Breaking change.** Every manifest entry's `files` field used to always be:

```json
"files": {
  "with_caption_webp": "...", "no_caption_webp": "...",
  "with_caption_avif": "...", "no_caption_avif": "..."
}
```

It is now:

```json
"files": { "format": "webp", "with_caption": "...", "no_caption": "..." }
```

`format` is one of `"webp"`, `"avif"`, `"png"`, `"jpeg"`, `"jpegxl"` (lowercase, matching
the radio button values - note `"jpegxl"` has no separator, while its file extension is
`.jxl`, mirroring how `"jpeg"` already maps to a `.jpg` extension). Old manifests from
before this change will fail to parse against the new
`ExportedFiles` struct - this is intentional (no dual-schema fallback was added, to avoid
silently reading stale data as if it were current); re-run extraction to get a manifest in
the new shape.

### JPEG XL: bundled libjxl encoder

JPEG XL is encoded in-process by the reference libjxl implementation, statically linked through the permissively licensed `gamut-jxl`/`gamut-jxl-sys` Rust wrapper. The app bundle therefore has no runtime dependency on Homebrew, `cjxl`, or another system library.

The fixed JPEG-style quality of 85 is converted with libjxl's public `JxlEncoderDistanceFromQuality` mapping to a Butteraugli distance of 1.45, preserving the quality setting previously passed to `cjxl -q 85`. The encoder emits a bare JPEG XL codestream beginning with `FF 0A`; `tests/export_formats.rs` exercises it unconditionally with the other four formats.

Building from source requires CMake and a C++ toolchain because libjxl is compiled and statically linked at build time. This does not affect `.dmg` or one-line-script installations: end users receive the already-built encoder inside `FigWizard.app`.

## Optional: verify crops with Codex (off by default)

Next to the Extract button is a checkbox: **"Verify crops with Codex (slower)"**. It is
**unchecked by default** and must be explicitly opted into every time, because it costs
real network access and real wall-clock time on top of an already CPU/time-heavy pipeline
(see "Extraction is slow" below).

When checked, before each detected object is exported the app shells out to the local
`codex` CLI (OpenAI's coding agent, used here purely as a multimodal judge - no code is
read or edited) with the current crop image and asks it to judge, per a structured JSON
schema, whether the crop is a clean, complete, standalone image of that object:

Finder-launched macOS apps do not inherit the same `PATH` as a terminal. FigWizard therefore resolves Codex from both `PATH` and common per-user install locations such as `~/.local/bin` and `~/.npm-global/bin`. For npm installs, it launches the platform-native Codex binary inside the package instead of the JavaScript wrapper, so the wrapper's `#!/usr/bin/env node` does not introduce the same `PATH` failure again.

- If Codex says the crop passes, it's exported as-is.
- If Codex flags an issue (cut off on some side, or including too much irrelevant extra content on some side), it also returns a suggested bounding-box correction in PDF points. For expansion-only feedback, the app adds a safety margin equal to 2% of the current bbox dimension on every side (width for left/right, height for top/bottom). Shrink-only or mixed expand/shrink feedback is applied exactly as suggested. The result is capped and clamped to sane bounds, re-rendered, and re-verified - **up to 3 total attempts per object** (1 initial check + up to 2 corrective re-renders; see `MAX_ATTEMPTS` in `src-tauri/src/verify/mod.rs` if you want to tune it).
- One verification pass runs per object, against its own (no-caption) bounding box; the
  corrected box is then reused for both the no-caption crop and the with-caption crop
  (re-unioned with the original caption box) - the with-caption variant is not verified
  separately.
- If Codex itself fails to run (binary missing/not authenticated, network hiccup, timeout,
  malformed output), that attempt is treated as a soft failure - it's consumed like any
  other failed attempt, and the pipeline falls back to the last-rendered crop rather than
  ever crashing or hanging the whole extraction job over it. Before starting a run with the
  checkbox on, the app does a quick upfront `codex --version` check and refuses to start
  (with a clear error) rather than silently doing nothing per-object if Codex isn't
  available at all.

**Per-object attempt counts are always visible when this feature was used**: `manifest.json` gets a `verification: { enabled, attempts, passed, last_issue, history }` field per object (absent entirely when the checkbox was off for that run), and the results list shows a small badge on each row - "✓ 1 try" (passed first try), "⟳ N tries" (passed after Codex-suggested corrections), or "⚠ N tries, still flagged" (never passed within the attempt budget). No badge is shown at all when verification wasn't enabled for that run.

**Full per-attempt history, not just the final outcome.** `verification.history` is an
array with one entry per real attempt (in order), each shaped like:

```json
{ "attempt": 1, "passed": false, "issue": "extra_content_top", "reason": "...", "bbox_adjustment_pt": [12.0, 0.0, 0.0, 0.0] }
```

(`bbox_adjustment_pt` is `[top, bottom, left, right]` in PDF points - Codex's *raw*
suggestion for that attempt, before the app's capping/clamping is applied; it's `null` on
a passed attempt, or on a soft failure where Codex itself couldn't be invoked/parsed - see
`"verification_error: ..."` issues below.) `attempts`/`passed`/`last_issue` are still
present as convenience fields summarizing the same data (`attempts == history.length`, `last_issue` mirrors the last history entry's `issue`) so anything that only needs the summary doesn't need to touch the array. Read `manifest.json` when the full reason and bounding-box adjustment for each attempt are needed.

## Known limitations / gaps (read before filing a bug)

- **Extraction is slow.** This is a CPU-bound pipeline: ONNX layout detection per page plus
  near-4K image encoding per object (AVIF is the slowest of the 5 shipped formats to
  encode; PNG/JPEG/WebP are faster), no GPU acceleration. A 15-page paper with ~17
  extracted objects takes on the order of ten-plus minutes on a laptop CPU with AVIF
  selected. The UI treats this as a real background job (live progress events,
  cancellable) rather than pretending it's fast — expect multi-minute runs on longer
  papers, and plan to let it run in the background. Note that since each run now only
  encodes ONE format instead of always doing both WebP and AVIF, actual per-object encode
  time is generally lower than in earlier versions of this app, particularly for the
  faster formats.
- **No code-snippet class.** The detection model (PP-DocLayoutV3) has no dedicated "code
  block" label distinct from prose. Inline source-code snippets get bucketed under the
  `algorithm` class (or missed entirely if they don't look like a boxed/numbered algorithm
  environment). If you need clean code-block extraction, this isn't it yet.
- **No quote/callout class.** Likewise, there's no model class for block quotes or callout
  boxes; nothing in the current label set maps to them, so they are never extracted (not
  even mis-classified — they're simply invisible to this pipeline).
- **Duplicate with/without-caption files when no caption is found.** Caption/number
  association (`figure_title` for figures/tables/algorithms, `formula_number` for formulas)
  is purely spatial (nearest box above/below or beside, same column, within a gap
  threshold). When no caption/number box is associable, the object is still exported with
  both a `_with-caption_` and a `_no-caption_` file — they're just byte-identical crops
  (the with-caption render is skipped and the no-caption bitmap is reused). This is
  expected behavior, not a bug: every object always has both files, `has_caption: false`
  in `manifest.json` tells you when they're duplicates.
- **No checksum verification on downloaded assets.** `download_model` streams the model,
  its config, and the PDFium archive straight to disk with no SHA-256 check against a known
  hash. Fine for a personal-use v1 pointed at a fixed, trusted URL; a TODO for later if this
  ever needs to be hardened.
- **Apple Silicon Macs only, macOS 15+** — this is an explicit scope decision, not a
  temporary gap. PDFium asset resolution is hardcoded to the arm64 build
  (`pdfium-mac-arm64.tgz`); Intel support is intentionally out of scope. `tauri.conf.json`
  sets `bundle.macOS.minimumSystemVersion` to `15.0` accordingly.
- **One extraction (or PDF open) at a time.** PDFium's library binding can only happen once
  per process (`pdfium-render`'s `Pdfium::bind_to_library` errors on a second call), so the
  app keeps a single shared `Pdfium` instance and hands it between commands rather than
  re-binding per call. The UI enforces this by disabling PDF/output selection while a job is
  running; there's no queueing of multiple extractions.
- **Per-page timing varies a lot** with page content (how many/how large the detected
  objects are) and with how much else is competing for CPU on the machine at the time - a
  12-page paper was observed taking well over an hour end-to-end during one `cargo tauri
  dev` manual test session (heavily CPU-contended machine at the time), vs. the ~15
  minutes/15-page figure quoted above from an otherwise-idle `cargo test` run. Treat both as
  data points, not guarantees, when judging a build's actual performance.
- **Codex crop verification (opt-in) adds a real network+time cost per object.** Each
  attempt is one `codex exec` call (~8-90s depending on model/reasoning effort and load);
  worst case (never passes) is 3 attempts per object. On a paper with dozens of objects this
  can add many minutes on top of the already-slow core pipeline - this is exactly why it
  defaults off. The cancel button is checked between attempts as well as between objects, so
  cancelling mid-verification is still responsive.
- **Codex's suggested bbox corrections are estimates, not measurements.** Codex only sees
  the rendered crop image (pixels); the prompt tells it the crop's current size in PDF
  points so it has a scale reference, but its correction is still a visual judgment call,
  not a pixel-precise measurement. Occasional over/under-correction (needing more than one
  retry to converge, or still being flagged after 3 attempts) is expected, not necessarily a
  bug - `last_issue`/`last_reason` in the manifest and the "still flagged" badge are there so
  you can spot and manually review those cases.
