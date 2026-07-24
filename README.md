# FigWizard

A macOS app that extracts figures, tables, formulas, and algorithm/pseudocode blocks from
academic paper PDFs, exporting each as a near-4K crop — with and without its caption — in
your choice of WebP, AVIF, PNG, JPEG, or JPEG XL.

Apple Silicon only, macOS 15+.

## Install

**macOS app** — downloads the latest release and installs it to `/Applications`:

```sh
curl -fsSL https://raw.githubusercontent.com/ziward-inc/fig-wizard/main/install.sh | bash
```

The release is ad-hoc signed but not notarized, so Gatekeeper will block a normal
double-click. Right-click → **Open** once, or run
`xattr -cr /Applications/FigWizard.app`. See [CONTRIBUTING.md](CONTRIBUTING.md) for what
full notarization would require.

**CLI binary via Cargo** — no `.app` bundle, no Dock icon, launches the same GUI from a
terminal:

```sh
git clone https://github.com/ziward-inc/fig-wizard.git
cd fig-wizard
cargo install --path src-tauri --locked
```

**From source**, for development:

```sh
pnpm install
pnpm tauri dev
```

On first run, the app downloads its detection model (~125MB) and PDFium — see
[CONTRIBUTING.md](CONTRIBUTING.md) for where those land and how dev builds can skip the
download.

## Usage

1. Drag a PDF onto the drop zone (or click "Choose PDF…").
2. Pick an output folder (defaults to `extracted/` next to the PDF).
3. Choose one output format — WebP, AVIF, PNG, JPEG, or JPEG XL (WebP is the default).
4. Click **Extract** and watch live per-page progress.
5. Browse results grouped by page; click **OPEN** to view the document's output folder in Finder.

Each run writes exactly two files per detected object (with-caption and no-caption), flat into
`<output_dir>/<pdf-stem>/` (filenames like `p04_figure-01_no-caption.webp` embed the page
number), plus a `manifest.json` describing every export (kind,
page, bounding box, score, caption association, file paths). Re-running with a different
format overwrites both the files and the manifest. See
[docs/TECHNICAL_DETAILS.md](docs/TECHNICAL_DETAILS.md) for the exact manifest schema.

### Optional: verify crops with Codex

An opt-in checkbox ("Verify crops with Codex") shells out to the local `codex` CLI to
judge whether each crop is clean and complete, requesting a bounding-box correction and
re-rendering up to twice if not. It's off by default — it costs real network time on top
of an already slow pipeline, and requires `codex` installed and authenticated. Details and
the manifest fields it adds are in
[docs/TECHNICAL_DETAILS.md](docs/TECHNICAL_DETAILS.md).

## Known limitations

- **Slow.** CPU-bound ONNX layout detection plus per-object image encoding, no GPU
  acceleration — expect multi-minute runs on longer papers.
- **No dedicated code-block or block-quote classes** in the detection model; inline code
  gets bucketed under "algorithm" or missed, and quotes/callouts aren't extracted at all.
- **One extraction at a time** — PDFium's binding can't be re-initialized mid-process, so
  the UI blocks starting a second job while one is running.
- **No checksum verification** on the downloaded model/PDFium assets.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for the frontend/backend architecture, build
requirements, and the release process.

## License

[Apache 2.0](LICENSE). Bundled/downloaded third-party components (PDFium, the
PP-DocLayoutV3 model, libjxl, the SUITE font) remain under their own licenses — see
[NOTICE](NOTICE).
