//! Dev-only end-to-end test: run the full render -> detect -> associate ->
//! crop -> export pipeline against a real PDF and inspect the manifest +
//! output files. Requires local dev assets (pdfium dylib + ONNX model)
//! that are gitignored - see src-tauri/binaries/pdfium and src-tauri/models.

use app_lib::detect::DEFAULT_SCORE_THRESH;
use app_lib::pdf::render::{init_pdfium, ClipRenderBudget};
use app_lib::pipeline::run::{process_pdf, PipelineEvent, ProcessPdfParams};
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
fn full_pipeline_on_attention_pdf() {
    let root = repo_root();
    let pdfium_dir = root.join("app/src-tauri/binaries/pdfium/lib");
    let model_path = root.join("app/src-tauri/models/PP-DocLayoutV3.onnx");
    let config_path = root.join("app/src-tauri/models/config.json");
    let pdf_path = root.join("phase0-spike/pdfs/attention.pdf");
    let output_dir = root.join("app/src-tauri/tests/output/attention_run");

    let _ = std::fs::remove_dir_all(&output_dir);

    let cfg: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
    let labels: Vec<String> = cfg["label_list"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();

    let pdfium = init_pdfium(&pdfium_dir).expect("init pdfium");
    let cancel = AtomicBool::new(false);

    let mut page_events = 0;
    let mut object_events = 0;
    let mut kinds_seen = std::collections::HashSet::new();

    let manifest = process_pdf(
        ProcessPdfParams {
            pdfium: &pdfium,
            pdf_path: &pdf_path,
            output_dir: &output_dir,
            model_path: &model_path,
            labels,
            score_thresh: DEFAULT_SCORE_THRESH,
            clip_budget: ClipRenderBudget::default(),
        },
        &cancel,
        |event| match event {
            PipelineEvent::PageDetected { page_index, counts_by_kind, .. } => {
                page_events += 1;
                println!("page {page_index} detected: {counts_by_kind:?}");
            }
            PipelineEvent::ObjectExported { id, kind, page_index } => {
                object_events += 1;
                kinds_seen.insert(kind.clone());
                println!("exported {id} ({kind}) on page {page_index}");
            }
            PipelineEvent::ExtractionComplete { manifest_path, object_count } => {
                println!("complete: {object_count} objects, manifest at {manifest_path:?}");
            }
        },
    )
    .expect("process_pdf failed");

    assert!(page_events > 0, "expected page-detected events");
    assert!(object_events > 0, "expected object-exported events");
    assert_eq!(manifest.objects.len(), object_events);
    assert!(kinds_seen.contains("table"), "expected at least one table on attention.pdf");
    assert!(kinds_seen.contains("figure"), "expected at least one figure on attention.pdf");
    assert!(kinds_seen.contains("formula"), "expected at least one formula on attention.pdf");

    // Every manifest entry's four files must actually exist on disk and be
    // non-trivially sized (i.e. not empty/corrupt encodes).
    for entry in &manifest.objects {
        for path in [
            &entry.files.with_caption_webp,
            &entry.files.no_caption_webp,
            &entry.files.with_caption_avif,
            &entry.files.no_caption_avif,
        ] {
            let meta = std::fs::metadata(path).unwrap_or_else(|e| panic!("missing file {path}: {e}"));
            assert!(meta.len() > 100, "suspiciously small file {path}: {} bytes", meta.len());
        }
    }

    let manifest_path = output_dir.join("attention/manifest.json");
    assert!(manifest_path.exists(), "manifest.json missing at {manifest_path:?}");

    println!("wrote {} objects across attention.pdf", manifest.objects.len());
}
