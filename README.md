# FigWizard

macOS-only Tauri app that extracts figures, tables, formulas, algorithm/pseudocode blocks,
and other non-body-text block-level regions from academic paper PDFs, exporting each as
a near-4K image (WebP, AVIF, PNG, JPEG, or JPEG XL - quality 85 for lossy formats), with
and without captions.

See the design plan for full architecture details.

Status: end-to-end app (Rust pipeline + Tauri commands + UI) is wired up and usable. See
`app/README.md` for how to run it and known limitations.
