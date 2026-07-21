# PDF Paper Image Extractor

A macOS-only Tauri app that extracts figures, tables, formulas, and algorithm/pseudocode
blocks from academic paper PDFs, exporting each as near-4K WebP and AVIF images (quality
85), with and without an associated caption.

## Running it

```sh
npm install
npm run tauri dev
```

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

## Using it

1. Drag a PDF onto the drop zone (or click "Choose PDF…").
2. Pick an output folder (defaults to an `extracted/` folder next to the PDF; override with
   "Choose output folder…").
3. Click "Extract" and watch the live per-page progress and running counts by kind.
4. When done, browse the results gallery (grouped by page, click a thumbnail for the full
   crop plus all 4 output file paths and a "Reveal in Finder" action).

Extraction writes to `<output_dir>/<pdf-stem>/page-NNNN/<kind>-NN_{with,no}-caption_q85.{webp,avif}`
plus a `manifest.json` describing every exported object (kind, page, bbox, score, caption
association, file paths).

## Known limitations / gaps (read before filing a bug)

- **Extraction is slow.** This is a CPU-bound pipeline: ONNX layout detection per page plus
  near-4K AVIF encoding per object, no GPU acceleration. A 15-page paper with ~17 extracted
  objects takes on the order of ten-plus minutes on a laptop CPU. The UI treats this as a
  real background job (live progress events, cancellable) rather than pretending it's fast
  — expect multi-minute runs on longer papers, and plan to let it run in the background.
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
  expected behavior, not a bug: every object always has all 4 files, `has_caption: false`
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
