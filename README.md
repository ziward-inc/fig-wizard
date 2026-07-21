# pdf-paper-image-extractor

macOS-only Tauri app that extracts figures, tables, formulas, algorithm/pseudocode blocks,
and other non-body-text block-level regions from academic paper PDFs, exporting each as
near-4K WebP and AVIF images (quality 85), with and without captions.

See the design plan for full architecture details.

Status: end-to-end app (Rust pipeline + Tauri commands + UI) is wired up and usable. See
`app/README.md` for how to run it and known limitations.
