# FigWizard

macOS-only Tauri app that extracts figures, tables, formulas, algorithm/pseudocode blocks,
and other non-body-text block-level regions from academic paper PDFs, exporting each as
a near-4K image (WebP, AVIF, PNG, JPEG, or JPEG XL - quality 85 for lossy formats), with
and without captions.

The Rust (Tauri v2) backend runs the extraction pipeline (PDFium rendering + PP-DocLayoutV3
ONNX layout detection); the UI is a Vite + React + TypeScript frontend built on shadcn/ui.

Status: end-to-end app (Rust pipeline + Tauri commands + UI) is wired up and usable. See
[`app/README.md`](app/README.md) for how to run it, the frontend stack, and known
limitations.

## Repo layout

- `app/` - the shipped Tauri app (Rust extraction pipeline + Vite/React UI). Start here.
- `phase0-spike/` - an early Python spike that validated the PP-DocLayoutV3 ONNX model
  against rendered arXiv pages before the Rust pipeline existed. Historical reference
  only; not part of the shipped app and not kept in sync with it.
