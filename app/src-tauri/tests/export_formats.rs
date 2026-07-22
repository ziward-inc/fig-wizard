//! Real, end-to-end regression coverage for the output-format picker: runs
//! the full render -> detect -> associate -> crop -> export pipeline against
//! a small real PDF once per `OutputFormat` variant, and for every exported
//! object checks that exactly 2 files exist (not the old always-4), each
//! with the right extension AND real magic bytes for that format - i.e.
//! this doesn't just check the code compiles, it confirms each encoder
//! produces a file a real decoder would recognize. Uses `ppo_mini.pdf`
//! (2 pages) rather than the 12-page `ppo.pdf`/`attention.pdf` so running
//! this once per format stays fast.
//!
//! `OutputFormat::JpegXl` is exercised too, but only if the `cjxl` CLI
//! (libjxl, `brew install jpeg-xl`) is actually available on this machine -
//! unlike the other 4 formats, which are fully self-contained via linked-in
//! Rust crates, JPEG XL shells out to an external system binary (see
//! `pipeline::export::encode_jpegxl` / `cjxl_available`). Rather than hard
//! failing the whole suite on a machine without libjxl installed, that case
//! is skipped with a clear printed message.

use figwizard_lib::detect::DEFAULT_SCORE_THRESH;
use figwizard_lib::pdf::render::{init_pdfium, ClipRenderBudget};
use figwizard_lib::pipeline::export::cjxl_available;
use figwizard_lib::pipeline::run::{process_pdf, PipelineEvent, ProcessPdfParams};
use figwizard_lib::pipeline::types::OutputFormat;
use pdfium_render::prelude::Pdfium;
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

/// Checks the first bytes of `data` against the real magic-byte signature
/// for `format`, and (best-effort) that a real decoder accepts it too.
fn assert_valid_magic_bytes(format: OutputFormat, data: &[u8]) {
    match format {
        OutputFormat::Png => {
            assert!(
                data.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]),
                "not a valid PNG signature: {:?}",
                &data[..data.len().min(16)]
            );
        }
        OutputFormat::Jpeg => {
            assert!(
                data.starts_with(&[0xFF, 0xD8, 0xFF]),
                "not a valid JPEG signature: {:?}",
                &data[..data.len().min(16)]
            );
        }
        OutputFormat::Webp => {
            assert!(
                data.len() >= 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WEBP",
                "not a valid WebP (RIFF/WEBP) signature: {:?}",
                &data[..data.len().min(16)]
            );
        }
        OutputFormat::Avif => {
            // ISOBMFF box: 4-byte size, "ftyp", then major brand - "avif"
            // for still images (this pipeline never emits avif sequences).
            assert!(
                data.len() >= 12 && &data[4..8] == b"ftyp" && &data[8..12] == b"avif",
                "not a valid AVIF (ftyp/avif) signature: {:?}",
                &data[..data.len().min(16)]
            );
        }
        OutputFormat::JpegXl => {
            // Raw JPEG XL codestream magic bytes - confirmed manually on
            // this machine: `cjxl input.png output.jxl -q 85` produces a
            // file starting with `FF 0A` (not the ISOBMFF `.jxl` container
            // form, which starts with a box header instead - `cjxl`'s
            // default output is the bare codestream).
            assert!(
                data.starts_with(&[0xFF, 0x0A]),
                "not a valid JPEG XL codestream signature: {:?}",
                &data[..data.len().min(16)]
            );
        }
    }

    // Cross-check with the `image` crate's own format sniffing where it
    // supports the format (avif support in `image` varies by version/
    // features, and this `image` crate has no JPEG XL decoder at all, so
    // those two are magic-bytes-only above).
    if !matches!(format, OutputFormat::Avif | OutputFormat::JpegXl) {
        image::load_from_memory(data).unwrap_or_else(|e| {
            panic!("`image` crate could not decode this {format:?} output: {e}")
        });
    }
}

/// `Pdfium::bind_to_library` can only succeed once per process (see
/// `commands.rs`'s own doc comment on `AppState::pdfium`), so every format
/// is exercised against ONE shared `Pdfium` instance within a single
/// `#[test]` function rather than one separate test function per format
/// (which `cargo test` would otherwise run as parallel threads in the same
/// process and hit `PdfiumLibraryBindingsAlreadyInitialized` on all but the
/// first).
fn run_for_format(pdfium: &Pdfium, root: &PathBuf, model_path: &PathBuf, labels: Vec<String>, format: OutputFormat) {
    let pdf_path = root.join("phase0-spike/pdfs/ppo_mini.pdf");
    let output_dir = root
        .join("app/src-tauri/tests/output/export_formats_run")
        .join(format.as_str());

    let _ = std::fs::remove_dir_all(&output_dir);

    let cancel = AtomicBool::new(false);

    let manifest = process_pdf(
        ProcessPdfParams {
            pdfium,
            pdf_path: &pdf_path,
            output_dir: &output_dir,
            model_path,
            labels,
            score_thresh: DEFAULT_SCORE_THRESH,
            clip_budget: ClipRenderBudget::default(),
            output_format: format,
            verify_with_codex: false,
        },
        &cancel,
        |event| {
            if let PipelineEvent::ExtractionComplete { object_count, .. } = event {
                println!("[{format:?}] complete: {object_count} objects");
            }
        },
    )
    .unwrap_or_else(|e| panic!("process_pdf failed for {format:?}: {e:#}"));

    assert!(!manifest.objects.is_empty(), "expected at least one object on ppo_mini.pdf");

    let expected_ext = format!(".{}", format.extension());
    for entry in &manifest.objects {
        assert_eq!(entry.files.format, format.as_str(), "manifest format string mismatch for {}", entry.id);

        for path in [&entry.files.with_caption, &entry.files.no_caption] {
            assert!(path.ends_with(&expected_ext), "expected {expected_ext} extension: {path}");
            if format.is_lossless() {
                assert!(!path.contains("_q85"), "PNG filenames shouldn't carry a quality suffix: {path}");
            } else {
                assert!(path.contains("_q85"), "lossy format filenames should carry _q85: {path}");
            }

            let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("reading {path}: {e}"));
            assert!(bytes.len() > 100, "suspiciously small {path}: {} bytes", bytes.len());
            assert_valid_magic_bytes(format, &bytes);
        }
    }

    println!("PASS: {format:?} - {} objects, all files valid", manifest.objects.len());
}

#[test]
fn export_all_formats_produce_valid_files() {
    let root = repo_root();
    let pdfium_dir = root.join("app/src-tauri/binaries/pdfium/lib");
    let model_path = root.join("app/src-tauri/models/PP-DocLayoutV3.onnx");
    let config_path = root.join("app/src-tauri/models/config.json");

    let cfg: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
    let labels: Vec<String> = cfg["label_list"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();

    let pdfium = init_pdfium(&pdfium_dir).expect("init pdfium");

    for format in [OutputFormat::Webp, OutputFormat::Avif, OutputFormat::Png, OutputFormat::Jpeg] {
        run_for_format(&pdfium, &root, &model_path, labels.clone(), format);
    }

    // JPEG XL depends on an external system binary (`cjxl`, not a linked-in
    // crate like the other 4 formats) - skip gracefully rather than failing
    // the whole suite on a machine that doesn't have libjxl installed.
    match cjxl_available() {
        Ok(version) => {
            println!("cjxl available ({version}) - running JpegXl case");
            run_for_format(&pdfium, &root, &model_path, labels.clone(), OutputFormat::JpegXl);
        }
        Err(e) => {
            println!(
                "SKIP: OutputFormat::JpegXl case - `cjxl` not available ({e}). \
Install via `brew install jpeg-xl` to include this case."
            );
        }
    }
}
