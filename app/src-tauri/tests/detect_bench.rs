//! Dev-only benchmark: isolate ONNX inference time (excluding PDF
//! rendering/resizing, and excluding AVIF/WebP encoding, which are not
//! affected by the execution provider) across every page of a real
//! multi-page PDF.
//!
//! This exists to answer a specific question: does enabling `ort`'s CoreML
//! execution provider (Cargo feature `coreml`) meaningfully speed up the
//! `model.run(...)` calls in `detect::DocLayoutModel::run`, compared to the
//! CPU-only default? Run it twice, comparing:
//!
//!   cargo test --release --test detect_bench -- --nocapture
//!   cargo test --release --features coreml --test detect_bench -- --nocapture
//!
//! Both runs also print the detections found on every page (label, score,
//! box) so the two runs' output can be diffed to check for correctness
//! regressions, not just timing.
//!
//! Requires local dev assets (pdfium dylib + ONNX model) that are
//! gitignored - see src-tauri/binaries/pdfium and src-tauria/models.

use app_lib::detect::{DocLayoutModel, DEFAULT_SCORE_THRESH, TARGET_SIZE};
use app_lib::pdf::render::{init_pdfium, render_page_for_detection, resize_for_model};
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

#[test]
fn bench_inference_only_attention_pdf() {
    let root = repo_root();
    let pdfium_dir = root.join("app/src-tauri/binaries/pdfium/lib");
    let model_path = root.join("app/src-tauri/models/PP-DocLayoutV3.onnx");
    let config_path = root.join("app/src-tauri/models/config.json");
    let pdf_path = root.join("phase0-spike/pdfs/attention.pdf");

    assert!(pdfium_dir.exists(), "pdfium dir missing: {pdfium_dir:?}");
    assert!(model_path.exists(), "model missing: {model_path:?}");
    assert!(pdf_path.exists(), "pdf missing: {pdf_path:?}");

    let cfg: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
    let labels: Vec<String> = cfg["label_list"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();

    let pdfium = init_pdfium(&pdfium_dir).expect("init pdfium");
    let doc = pdfium
        .load_pdf_from_file(&pdf_path, None)
        .expect("load pdf");

    let page_count = doc.pages().len();
    println!("pdf: {pdf_path:?}, {page_count} pages");

    #[cfg(feature = "coreml")]
    println!("execution provider: CoreML (feature `coreml` enabled), falling back to CPU per-op/on registration failure");
    #[cfg(not(feature = "coreml"))]
    println!("execution provider: CPU only (feature `coreml` NOT enabled)");

    let load_start = Instant::now();
    let mut model = DocLayoutModel::load(&model_path, labels).expect("load model");
    println!("model load time: {:?}", load_start.elapsed());

    // Pre-render and pre-resize ALL pages up front so the timed region below
    // contains *only* `model.run(...)` calls - no rendering, no resizing, no
    // I/O.
    let mut inputs = Vec::new();
    for i in 0..page_count {
        let page = doc.pages().get(i).expect("get page");
        let (page_img, _geometry) = render_page_for_detection(&page, 200.0).expect("render page");
        let (resized, scale_h, scale_w) = resize_for_model(&page_img, TARGET_SIZE.1, TARGET_SIZE.0);
        inputs.push((i, resized, scale_h, scale_w));
    }

    // One warm-up run so model/session lazy-init (e.g. CoreML graph
    // compilation, which happens on first run, not on session creation) is
    // excluded from the steady-state per-page timings.
    {
        let (_, resized, scale_h, scale_w) = &inputs[0];
        let _ = model
            .run(resized, *scale_h, *scale_w, DEFAULT_SCORE_THRESH)
            .expect("warm-up run");
    }

    let mut total_inference = Duration::ZERO;
    let mut per_page = Vec::new();

    for (page_index, resized, scale_h, scale_w) in &inputs {
        let start = Instant::now();
        let dets = model
            .run(resized, *scale_h, *scale_w, DEFAULT_SCORE_THRESH)
            .expect("run detection");
        let elapsed = start.elapsed();
        total_inference += elapsed;

        println!(
            "page {page_index}: {} dets, inference={:?}",
            dets.len(),
            elapsed
        );
        for d in &dets {
            println!(
                "    {:20} score={:.3} px=[{:.0},{:.0},{:.0},{:.0}]",
                d.label, d.score, d.px_x0, d.px_y0, d.px_x1, d.px_y1
            );
        }
        per_page.push((*page_index, dets.len(), elapsed));
    }

    println!("---");
    println!(
        "TOTAL inference time across {} pages: {:?} (avg {:?}/page)",
        page_count,
        total_inference,
        total_inference / page_count as u32
    );
}
