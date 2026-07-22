//! Dev-only end-to-end test: run the full render -> detect -> associate ->
//! crop -> export pipeline against a real PDF and inspect the manifest +
//! output files. Requires local dev assets (pdfium dylib + ONNX model)
//! that are gitignored - see src-tauri/binaries/pdfium and src-tauri/models.

use app_lib::detect::DEFAULT_SCORE_THRESH;
use app_lib::pdf::render::{init_pdfium, ClipRenderBudget};
use app_lib::pipeline::run::{process_pdf, PipelineEvent, ProcessPdfParams};
use app_lib::pipeline::types::OutputFormat;
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
            output_format: OutputFormat::Webp,
            verify_with_codex: false,
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

    // Every manifest entry's two files (with/without caption, in the
    // run's single selected format - WebP here) must actually exist on
    // disk and be non-trivially sized (i.e. not empty/corrupt encodes).
    for entry in &manifest.objects {
        assert_eq!(entry.files.format, "webp", "unexpected format for {}", entry.id);
        for path in [&entry.files.with_caption, &entry.files.no_caption] {
            let meta = std::fs::metadata(path).unwrap_or_else(|e| panic!("missing file {path}: {e}"));
            assert!(meta.len() > 100, "suspiciously small file {path}: {} bytes", meta.len());
            assert!(path.ends_with(".webp"), "expected .webp extension: {path}");
        }
    }

    let manifest_path = output_dir.join("attention/manifest.json");
    assert!(manifest_path.exists(), "manifest.json missing at {manifest_path:?}");

    println!("wrote {} objects across attention.pdf", manifest.objects.len());
}

/// Dev-only, network-touching end-to-end test: runs the full pipeline
/// against ppo.pdf (smaller/faster than attention.pdf, algorithm-heavy)
/// WITH the optional Codex crop-verification pass turned on, to confirm
/// real `codex exec` calls happen, the manifest records real per-object
/// attempt counts, and the feature never aborts the whole run even if some
/// individual object fails to pass verification. Marked `#[ignore]` since
/// it costs real wall-clock time and a live Codex CLI/network round-trip
/// per object - run explicitly with
/// `cargo test --test full_pipeline verify_with_codex_on_ppo_pdf -- --ignored --nocapture`.
#[test]
#[ignore]
fn verify_with_codex_on_ppo_pdf() {
    let root = repo_root();
    let pdfium_dir = root.join("app/src-tauri/binaries/pdfium/lib");
    let model_path = root.join("app/src-tauri/models/PP-DocLayoutV3.onnx");
    let config_path = root.join("app/src-tauri/models/config.json");
    let pdf_path = root.join("phase0-spike/pdfs/ppo.pdf");
    let output_dir = root.join("app/src-tauri/tests/output/ppo_verify_run");

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

    let manifest = process_pdf(
        ProcessPdfParams {
            pdfium: &pdfium,
            pdf_path: &pdf_path,
            output_dir: &output_dir,
            model_path: &model_path,
            labels,
            score_thresh: DEFAULT_SCORE_THRESH,
            clip_budget: ClipRenderBudget::default(),
            output_format: OutputFormat::Webp,
            verify_with_codex: true,
        },
        &cancel,
        |event| match event {
            PipelineEvent::PageDetected { page_index, counts_by_kind, .. } => {
                println!("page {page_index} detected: {counts_by_kind:?}");
            }
            PipelineEvent::ObjectExported { id, kind, page_index } => {
                println!("exported {id} ({kind}) on page {page_index}");
            }
            PipelineEvent::ExtractionComplete { manifest_path, object_count } => {
                println!("complete: {object_count} objects, manifest at {manifest_path:?}");
            }
        },
    )
    .expect("process_pdf with verify_with_codex failed");

    assert!(!manifest.objects.is_empty(), "expected at least one object on ppo.pdf");

    for entry in &manifest.objects {
        let v = entry
            .verification
            .as_ref()
            .unwrap_or_else(|| panic!("object {} missing verification info", entry.id));
        assert!(v.enabled, "verification.enabled should be true when the feature was on");
        assert!(v.attempts >= 1, "expected at least 1 attempt for {}", entry.id);
        assert_eq!(
            v.history.len() as u32,
            v.attempts,
            "history length should match attempts for {}",
            entry.id
        );
        assert_eq!(
            v.history.last().map(|a| a.issue.clone()),
            v.last_issue,
            "history's last entry should match last_issue for {}",
            entry.id
        );
        println!(
            "{}: passed={} attempts={} last_issue={:?}",
            entry.id, v.passed, v.attempts, v.last_issue
        );
        for a in &v.history {
            println!(
                "  attempt {}: passed={} issue={} reason={} bbox_adjustment_pt={:?}",
                a.attempt, a.passed, a.issue, a.reason, a.bbox_adjustment_pt
            );
        }
    }
}
