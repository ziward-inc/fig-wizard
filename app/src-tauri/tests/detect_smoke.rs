//! Dev-only smoke test: render a real PDF page, run the ONNX detector, and
//! print/save results for manual inspection. Requires local dev assets
//! (pdfium dylib + ONNX model) that are gitignored - see
//! src-tauri/binaries/pdfium and src-tauri/models.

use figwizard_lib::detect::{DocLayoutModel, DEFAULT_SCORE_THRESH, TARGET_SIZE};
use figwizard_lib::pdf::render::{
    init_pdfium, pixel_box_to_pdf_points, render_clip, render_page_for_detection, resize_for_model,
    ClipRenderBudget,
};
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    // src-tauri/ -> app/ -> worktree root
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

#[test]
fn detect_ppo_algorithm_page() {
    let root = repo_root();
    let pdfium_dir = root.join("app/src-tauri/binaries/pdfium/lib");
    let model_path = root.join("app/src-tauri/models/PP-DocLayoutV3.onnx");
    let config_path = root.join("app/src-tauri/models/config.json");
    let pdf_path = root.join("phase0-spike/pdfs/ppo.pdf");

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

    // Page index 4 (0-based) = ppo.pdf page 5, contains an algorithm block
    // per phase0-spike/render_pages.py's TARGETS list.
    let page = doc.pages().get(4).expect("get page 4");

    let (page_img, geometry) = render_page_for_detection(&page, 200.0).expect("render page");
    println!(
        "rendered page: {}x{} px, page geometry {}x{} pt",
        page_img.width(),
        page_img.height(),
        geometry.width_pt,
        geometry.height_pt
    );

    let (resized, scale_h, scale_w) = resize_for_model(&page_img, TARGET_SIZE.1, TARGET_SIZE.0);

    let mut model = DocLayoutModel::load(&model_path, labels).expect("load model");
    let dets = model
        .run(&resized, scale_h, scale_w, DEFAULT_SCORE_THRESH)
        .expect("run detection");

    println!("detections: {}", dets.len());
    let mut found_algorithm = false;
    for d in &dets {
        let bbox_pt = pixel_box_to_pdf_points(d.px_x0, d.px_y0, d.px_x1, d.px_y1, 200.0, geometry.height_pt);
        println!(
            "  {:20} score={:.3} px=[{:.0},{:.0},{:.0},{:.0}] pt=[{:.1},{:.1},{:.1},{:.1}]",
            d.label, d.score, d.px_x0, d.px_y0, d.px_x1, d.px_y1, bbox_pt.x0, bbox_pt.y0, bbox_pt.x1, bbox_pt.y1
        );
        if d.label == "algorithm" {
            found_algorithm = true;
        }
    }

    assert!(!dets.is_empty(), "expected at least one detection");
    assert!(found_algorithm, "expected an 'algorithm' class detection on ppo.pdf page 5");

    // Render the algorithm box at near-4K directly from vector content, and
    // save it so a human can visually confirm the crop lines up.
    let algo = dets.iter().find(|d| d.label == "algorithm").unwrap();
    let bbox_pt = pixel_box_to_pdf_points(
        algo.px_x0,
        algo.px_y0,
        algo.px_x1,
        algo.px_y1,
        200.0,
        geometry.height_pt,
    );
    let crop = render_clip(&page, bbox_pt, ClipRenderBudget::default()).expect("render clip");
    println!("clip render: {}x{} px", crop.width(), crop.height());

    let long_side = crop.width().max(crop.height());
    assert!(
        long_side >= 3000,
        "expected near-4K long side, got {long_side}"
    );

    let out_dir = root.join("app/src-tauri/tests/output");
    std::fs::create_dir_all(&out_dir).unwrap();
    let out_path = out_dir.join("ppo_algorithm_crop.png");
    crop.save(&out_path).expect("save crop png");
    println!("saved crop to {out_path:?}");
}
