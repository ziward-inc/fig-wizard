//! Fast, targeted smoke test for the optional Codex crop-verification
//! feature: verifies ONE known real object (the algorithm box on ppo.pdf
//! page 5, same object `tests/detect_smoke.rs` already exercises) rather
//! than running the full 12-page pipeline, so it completes in well under a
//! minute of real `codex exec` calls instead of the 10+ minutes a full-PDF
//! run costs. Requires local dev assets (pdfium dylib + ONNX model) and a
//! working, authenticated `codex` CLI on PATH - run explicitly with
//! `cargo test --test verify_smoke -- --ignored --nocapture`.

use figwizard_lib::detect::{DocLayoutModel, DEFAULT_SCORE_THRESH, TARGET_SIZE};
use figwizard_lib::pdf::render::{
    init_pdfium, pixel_box_to_pdf_points, render_page_for_detection, resize_for_model,
    ClipRenderBudget,
};
use figwizard_lib::verify::{codex_available, verify_and_correct_crop, MAX_ATTEMPTS};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

#[test]
#[ignore]
fn verify_single_algorithm_crop() {
    let version = codex_available().expect("codex CLI should be available and runnable");
    println!("codex available: {version}");

    let root = repo_root();
    let pdfium_dir = root.join("app/src-tauri/binaries/pdfium/lib");
    let model_path = root.join("app/src-tauri/models/PP-DocLayoutV3.onnx");
    let config_path = root.join("app/src-tauri/models/config.json");
    let pdf_path = root.join("phase0-spike/pdfs/ppo.pdf");

    let cfg: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
    let labels: Vec<String> = cfg["label_list"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();

    let pdfium = init_pdfium(&pdfium_dir).expect("init pdfium");
    let doc = pdfium.load_pdf_from_file(&pdf_path, None).expect("load pdf");
    let page = doc.pages().get(4).expect("get page 4"); // 0-based -> ppo.pdf page 5

    let (page_img, geometry) = render_page_for_detection(&page, 200.0).expect("render page");
    let (resized, scale_h, scale_w) = resize_for_model(&page_img, TARGET_SIZE.1, TARGET_SIZE.0);

    let mut model = DocLayoutModel::load(&model_path, labels).expect("load model");
    let dets = model
        .run(&resized, scale_h, scale_w, DEFAULT_SCORE_THRESH)
        .expect("run detection");

    let algo = dets
        .iter()
        .find(|d| d.label == "algorithm")
        .expect("expected an algorithm detection on ppo.pdf page 5");
    let bbox_pt = pixel_box_to_pdf_points(
        algo.px_x0,
        algo.px_y0,
        algo.px_x1,
        algo.px_y1,
        200.0,
        geometry.height_pt,
    );
    println!("initial bbox_pt: {bbox_pt:?}");

    let work_dir = std::env::temp_dir().join("pdf-extractor-verify-smoke-test");
    let cancel = AtomicBool::new(false);

    let start = std::time::Instant::now();
    let (_img, corrected_bbox, outcome) = verify_and_correct_crop(
        &page,
        "algorithm",
        bbox_pt,
        ClipRenderBudget::default(),
        MAX_ATTEMPTS,
        &work_dir,
        &cancel,
    )
    .expect("verify_and_correct_crop failed");

    println!(
        "RESULT: passed={} attempts={} last_issue={:?} last_reason={:?} elapsed={:?}",
        outcome.passed, outcome.attempts, outcome.last_issue, outcome.last_reason, start.elapsed()
    );
    println!("corrected bbox_pt: {corrected_bbox:?}");
    for a in &outcome.history {
        println!(
            "  attempt {}: passed={} issue={} reason={} bbox_adjustment_pt={:?}",
            a.attempt, a.passed, a.issue, a.reason, a.bbox_adjustment_pt
        );
    }

    assert!(outcome.attempts >= 1, "expected at least 1 real attempt");
    assert!(
        !outcome
            .last_issue
            .as_deref()
            .unwrap_or("")
            .starts_with("verification_error"),
        "expected a real Codex response, not an error: {:?}",
        outcome.last_issue
    );

    // The new per-attempt history should be populated and consistent with
    // the derived summary fields (see `verify::finish_outcome`).
    assert!(!outcome.history.is_empty(), "expected at least 1 history entry");
    assert_eq!(
        outcome.history.len() as u32,
        outcome.attempts,
        "history length should match attempts"
    );
    assert_eq!(
        outcome.history.last().map(|a| a.issue.clone()),
        outcome.last_issue,
        "history's last entry should match last_issue"
    );
    let last_attempt = outcome.history.last().expect("checked non-empty above");
    assert_eq!(last_attempt.passed, outcome.passed, "history's last entry should match overall passed");
    if last_attempt.passed {
        assert!(
            last_attempt.bbox_adjustment_pt.is_none(),
            "a passed attempt should have no bbox adjustment recorded"
        );
    }
}
