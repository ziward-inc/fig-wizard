//! Tauri command surface: wires the pure-Rust pipeline (`pipeline::run`)
//! plus PDFium/model asset resolution and first-run download into actual
//! `#[tauri::command]`s the frontend can call.
//!
//! Asset resolution strategy (see `resolve_pdfium_dir` / `resolve_model_and_labels`):
//! production always looks in the Tauri app-data-dir (populated by
//! `download_model`); if running a debug build (`cargo tauri dev`) and the
//! app-data-dir copy isn't there yet, we fall back to the gitignored local
//! dev assets under `src-tauri/models` / `src-tauri/binaries/pdfium` for
//! developer convenience. That dev fallback is compiled out of release
//! builds (`cfg!(debug_assertions)`), so a packaged app never depends on
//! paths that only exist on the build machine.

use pdfium_render::prelude::Pdfium;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

use crate::detect::DEFAULT_SCORE_THRESH;
use crate::pdf::render::{init_pdfium, ClipRenderBudget};
use crate::pipeline::run::{process_pdf, PipelineEvent, ProcessPdfParams};
use crate::pipeline::types::Manifest;

const MODEL_URL: &str =
    "https://huggingface.co/alex-dinh/PP-DocLayoutV3-ONNX/resolve/main/PP-DocLayoutV3.onnx";
const MODEL_CONFIG_URL: &str =
    "https://huggingface.co/alex-dinh/PP-DocLayoutV3-ONNX/resolve/main/config.json";
const PDFIUM_URL: &str =
    "https://github.com/bblanchon/pdfium-binaries/releases/download/chromium/7961/pdfium-mac-arm64.tgz";
const PDFIUM_DYLIB_NAME: &str = "libpdfium.dylib";
const MODEL_FILE_NAME: &str = "PP-DocLayoutV3.onnx";
const MODEL_CONFIG_FILE_NAME: &str = "config.json";

/// Tracks in-flight extraction jobs so `cancel_extraction` can flip the
/// right `AtomicBool`. The `Arc<Mutex<..>>` (rather than putting the whole
/// state behind Tauri's managed-state mutex) lets us clone just the map
/// handle into the `spawn_blocking` closure to remove the entry when done.
///
/// `pdfium` holds a single lazily-initialized `Pdfium` instance shared by
/// every command that needs one. This is NOT just a perf optimization:
/// `Pdfium::bind_to_library` can only succeed ONCE per process - a second
/// call (e.g. `open_pdf` binding once, then `run_extraction` binding again)
/// fails with `PdfiumLibraryBindingsAlreadyInitialized`. `Pdfium` is `Send +
/// Sync` (see pdfium-render's own unsafe impls), so it's safe to hand the
/// owned instance across the `spawn_blocking` thread boundary for the
/// duration of an extraction and hand it back to the slot afterwards.
#[derive(Default, Clone)]
pub struct AppState {
    jobs: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
    pdfium: Arc<Mutex<Option<Pdfium>>>,
}

/// Returns the shared `Pdfium` instance, initializing it on first use.
/// Failures (e.g. library not downloaded yet) are never cached - the next
/// call will simply try `init_pdfium` again once assets are available.
fn take_pdfium(app: &AppHandle, state: &AppState) -> Result<Pdfium, String> {
    let mut guard = state.pdfium.lock().unwrap();
    if let Some(p) = guard.take() {
        return Ok(p);
    }
    drop(guard);
    let dir = resolve_pdfium_dir(app)?;
    init_pdfium(&dir).map_err(|e| format!("Failed to init PDFium: {e}"))
}

fn return_pdfium(state: &AppState, pdfium: Pdfium) {
    *state.pdfium.lock().unwrap() = Some(pdfium);
}

#[derive(Debug, Serialize)]
pub struct PdfInfo {
    pub path: String,
    pub page_count: u32,
}

#[derive(Debug, Serialize)]
pub struct ModelStatus {
    pub model_present: bool,
    pub pdfium_present: bool,
    pub using_dev_assets: bool,
}

fn app_data_models_dir(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|e| format!("resolving app data dir: {e}"))?
        .join("models"))
}

fn app_data_pdfium_dir(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|e| format!("resolving app data dir: {e}"))?
        .join("pdfium")
        .join("lib"))
}

/// Dev-only fallback: `src-tauri/models` next to this crate's `Cargo.toml`.
/// `CARGO_MANIFEST_DIR` is a build-machine path, so this is only consulted
/// in debug builds - see module docs.
fn dev_models_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models")
}

fn dev_pdfium_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("binaries")
        .join("pdfium")
        .join("lib")
}

fn model_ready(dir: &Path) -> bool {
    dir.join(MODEL_FILE_NAME).is_file() && dir.join(MODEL_CONFIG_FILE_NAME).is_file()
}

fn pdfium_ready(dir: &Path) -> bool {
    dir.join(PDFIUM_DYLIB_NAME).is_file()
}

fn resolve_pdfium_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let prod = app_data_pdfium_dir(app)?;
    if pdfium_ready(&prod) {
        return Ok(prod);
    }
    if cfg!(debug_assertions) {
        let dev = dev_pdfium_dir();
        if pdfium_ready(&dev) {
            return Ok(dev);
        }
    }
    Err("PDFium library not found. Use the \"Download model\" button first.".to_string())
}

fn resolve_model_and_labels(app: &AppHandle) -> Result<(PathBuf, Vec<String>), String> {
    let prod = app_data_models_dir(app)?;
    let models_dir = if model_ready(&prod) {
        prod
    } else if cfg!(debug_assertions) && model_ready(&dev_models_dir()) {
        dev_models_dir()
    } else {
        return Err("Detection model not found. Use the \"Download model\" button first.".to_string());
    };

    let config_path = models_dir.join(MODEL_CONFIG_FILE_NAME);
    let raw = std::fs::read_to_string(&config_path)
        .map_err(|e| format!("reading {config_path:?}: {e}"))?;
    let cfg: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("parsing {config_path:?}: {e}"))?;
    let labels: Vec<String> = cfg["label_list"]
        .as_array()
        .ok_or_else(|| format!("{config_path:?} missing label_list"))?
        .iter()
        .map(|v| v.as_str().unwrap_or_default().to_string())
        .collect();

    Ok((models_dir.join(MODEL_FILE_NAME), labels))
}

/// Validates the file exists/looks like a PDF and returns its page count by
/// briefly opening it with PDFium. Uses the shared `Pdfium` instance from
/// `AppState` (see its doc comment) rather than re-binding the library.
#[tauri::command]
pub fn open_pdf(app: AppHandle, state: tauri::State<'_, AppState>, path: String) -> Result<PdfInfo, String> {
    let p = PathBuf::from(&path);
    if !p.is_file() {
        return Err(format!("File not found: {path}"));
    }
    let looks_like_pdf = p
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("pdf"))
        .unwrap_or(false);
    if !looks_like_pdf {
        return Err("Selected file is not a .pdf".to_string());
    }

    let pdfium = take_pdfium(&app, &state)?;
    let result = pdfium
        .load_pdf_from_file(&p, None)
        .map(|doc| doc.pages().len() as u32)
        .map_err(|e| format!("Failed to open PDF (is it valid/unencrypted?): {e}"));
    return_pdfium(&state, pdfium);

    let page_count = result?;
    Ok(PdfInfo { path, page_count })
}

#[tauri::command]
pub async fn pick_pdf_file(app: AppHandle) -> Result<Option<String>, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .add_filter("PDF", &["pdf"])
        .pick_file(move |picked| {
            let _ = tx.send(picked);
        });
    let picked = rx.await.map_err(|e| e.to_string())?;
    Ok(picked.map(|p| p.to_string()))
}

#[tauri::command]
pub async fn pick_output_dir(app: AppHandle) -> Result<Option<String>, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().pick_folder(move |picked| {
        let _ = tx.send(picked);
    });
    let picked = rx.await.map_err(|e| e.to_string())?;
    Ok(picked.map(|p| p.to_string()))
}

#[tauri::command]
pub fn model_status(app: AppHandle) -> Result<ModelStatus, String> {
    let prod_models = app_data_models_dir(&app)?;
    let prod_pdfium = app_data_pdfium_dir(&app)?;
    let prod_model_ok = model_ready(&prod_models);
    let prod_pdfium_ok = pdfium_ready(&prod_pdfium);

    if prod_model_ok && prod_pdfium_ok {
        return Ok(ModelStatus {
            model_present: true,
            pdfium_present: true,
            using_dev_assets: false,
        });
    }

    if cfg!(debug_assertions) {
        let dev_model_ok = model_ready(&dev_models_dir());
        let dev_pdfium_ok = pdfium_ready(&dev_pdfium_dir());
        return Ok(ModelStatus {
            model_present: prod_model_ok || dev_model_ok,
            pdfium_present: prod_pdfium_ok || dev_pdfium_ok,
            using_dev_assets: (!prod_model_ok && dev_model_ok) || (!prod_pdfium_ok && dev_pdfium_ok),
        });
    }

    Ok(ModelStatus {
        model_present: prod_model_ok,
        pdfium_present: prod_pdfium_ok,
        using_dev_assets: false,
    })
}

/// Starts extraction on a background thread (`spawn_blocking` - PDFium/ORT
/// are not async-friendly) and returns immediately with a job id; progress
/// comes back via `page-detected` / `object-exported` / `extraction-complete`
/// (or `extraction-error`) events, all tagged with `jobId`.
#[tauri::command]
pub async fn run_extraction(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    pdf_path: String,
    output_dir: String,
) -> Result<String, String> {
    let (model_path, labels) = resolve_model_and_labels(&app)?;
    let pdfium = take_pdfium(&app, &state)?;

    // Make sure the asset protocol can serve images back out of wherever
    // the user pointed the output dir, before extraction even starts.
    let _ = app.asset_protocol_scope().allow_directory(&output_dir, true);

    let job_id = uuid::Uuid::new_v4().to_string();
    let cancel = Arc::new(AtomicBool::new(false));
    state.jobs.lock().unwrap().insert(job_id.clone(), cancel.clone());

    let jobs = state.jobs.clone();
    let pdfium_slot = state.pdfium.clone();
    let job_id_done = job_id.clone();
    let job_id_events = job_id.clone();
    let app_events = app.clone();
    let app_done = app.clone();

    let pdf_path_buf = PathBuf::from(&pdf_path);
    let output_dir_buf = PathBuf::from(&output_dir);

    tauri::async_runtime::spawn_blocking(move || {
        let outcome = (|| -> anyhow::Result<()> {
            process_pdf(
                ProcessPdfParams {
                    pdfium: &pdfium,
                    pdf_path: &pdf_path_buf,
                    output_dir: &output_dir_buf,
                    model_path: &model_path,
                    labels,
                    score_thresh: DEFAULT_SCORE_THRESH,
                    clip_budget: ClipRenderBudget::default(),
                },
                &cancel,
                move |event| {
                    let (name, payload) = match event {
                        PipelineEvent::PageDetected {
                            page_index,
                            page_count,
                            counts_by_kind,
                        } => (
                            "page-detected",
                            serde_json::json!({
                                "jobId": job_id_events,
                                "pageIndex": page_index,
                                "pageCount": page_count,
                                "countsByKind": counts_by_kind,
                            }),
                        ),
                        PipelineEvent::ObjectExported { id, kind, page_index } => (
                            "object-exported",
                            serde_json::json!({
                                "jobId": job_id_events,
                                "id": id,
                                "kind": kind,
                                "pageIndex": page_index,
                            }),
                        ),
                        PipelineEvent::ExtractionComplete {
                            manifest_path,
                            object_count,
                        } => (
                            "extraction-complete",
                            serde_json::json!({
                                "jobId": job_id_events,
                                "manifestPath": manifest_path.to_string_lossy(),
                                "objectCount": object_count,
                            }),
                        ),
                    };
                    let _ = app_events.emit(name, payload);
                },
            )?;
            Ok(())
        })();

        // Hand the Pdfium instance back to the shared slot regardless of
        // outcome, so the next command (another extraction, or open_pdf)
        // doesn't have to (and can't safely) re-bind the library.
        *pdfium_slot.lock().unwrap() = Some(pdfium);

        if let Err(e) = outcome {
            let _ = app_done.emit(
                "extraction-error",
                serde_json::json!({ "jobId": job_id_done, "message": format!("{e:#}") }),
            );
        }
        jobs.lock().unwrap().remove(&job_id_done);
    });

    Ok(job_id)
}

#[tauri::command]
pub fn cancel_extraction(state: tauri::State<'_, AppState>, job_id: String) -> Result<(), String> {
    let jobs = state.jobs.lock().unwrap();
    match jobs.get(&job_id) {
        Some(flag) => {
            flag.store(true, Ordering::Relaxed);
            Ok(())
        }
        None => Err(format!("Unknown or already-finished job id: {job_id}")),
    }
}

/// Reads back `<output_dir>/<pdf_stem>/manifest.json` written by a prior
/// `run_extraction` call, and (re-)grants the asset protocol access to that
/// directory so `convertFileSrc` can load the crop thumbnails - scope is
/// per-session, so this needs to run again after an app restart too.
#[tauri::command]
pub fn list_results(app: AppHandle, output_dir: String, pdf_stem: String) -> Result<Manifest, String> {
    let doc_dir = PathBuf::from(&output_dir).join(&pdf_stem);
    let manifest_path = doc_dir.join("manifest.json");
    let raw = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("reading {manifest_path:?}: {e}"))?;
    let manifest: Manifest =
        serde_json::from_str(&raw).map_err(|e| format!("parsing {manifest_path:?}: {e}"))?;

    let _ = app.asset_protocol_scope().allow_directory(&doc_dir, true);

    Ok(manifest)
}

#[tauri::command]
pub fn reveal_in_finder(app: AppHandle, path: String) -> Result<(), String> {
    app.opener()
        .reveal_item_in_dir(&path)
        .map_err(|e| format!("Failed to reveal {path}: {e}"))
}

/// Downloads the ONNX model + its config, and the macOS arm64 PDFium dylib
/// (extracted from its `.tgz` release asset), into the app-data-dir so a
/// packaged production build doesn't need ~130MB+ of weights bundled in.
/// Emits `model-download-progress` events (`{ stage, downloaded, total }`)
/// as each file streams in. No checksum verification yet - TODO for a
/// future pass, not critical for personal-use v1.
#[tauri::command]
pub async fn download_model(app: AppHandle) -> Result<(), String> {
    let models_dir = app_data_models_dir(&app)?;
    let pdfium_root_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("pdfium");

    std::fs::create_dir_all(&models_dir)
        .map_err(|e| format!("creating {models_dir:?}: {e}"))?;
    std::fs::create_dir_all(&pdfium_root_dir)
        .map_err(|e| format!("creating {pdfium_root_dir:?}: {e}"))?;

    let client = reqwest::Client::builder()
        .build()
        .map_err(|e| format!("building HTTP client: {e}"))?;

    download_file_with_progress(
        &app,
        &client,
        MODEL_CONFIG_URL,
        &models_dir.join(MODEL_CONFIG_FILE_NAME),
        "config",
    )
    .await?;

    download_file_with_progress(
        &app,
        &client,
        MODEL_URL,
        &models_dir.join(MODEL_FILE_NAME),
        "model",
    )
    .await?;

    let tgz_path = pdfium_root_dir.join("pdfium-mac-arm64.tgz");
    download_file_with_progress(&app, &client, PDFIUM_URL, &tgz_path, "pdfium").await?;

    let extract_dir = pdfium_root_dir.clone();
    let extract_tgz = tgz_path.clone();
    tokio::task::spawn_blocking(move || extract_pdfium_dylib(&extract_tgz, &extract_dir))
        .await
        .map_err(|e| format!("extraction task panicked: {e}"))?
        .map_err(|e| format!("extracting PDFium archive: {e}"))?;

    let _ = std::fs::remove_file(&tgz_path);

    Ok(())
}

async fn download_file_with_progress(
    app: &AppHandle,
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    stage: &str,
) -> Result<(), String> {
    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;

    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("GET {url} failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("GET {url} returned HTTP {}", resp.status()));
    }
    let total = resp.content_length().unwrap_or(0);

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut file = tokio::fs::File::create(dest)
        .await
        .map_err(|e| format!("creating {dest:?}: {e}"))?;

    let mut stream = resp.bytes_stream();
    let mut downloaded: u64 = 0;
    let mut last_emitted: u64 = 0;

    let _ = app.emit(
        "model-download-progress",
        serde_json::json!({ "stage": stage, "downloaded": 0, "total": total }),
    );

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("downloading {url}: {e}"))?;
        file.write_all(&chunk)
            .await
            .map_err(|e| format!("writing {dest:?}: {e}"))?;
        downloaded += chunk.len() as u64;

        if downloaded - last_emitted >= 512 * 1024 || downloaded == total {
            last_emitted = downloaded;
            let _ = app.emit(
                "model-download-progress",
                serde_json::json!({ "stage": stage, "downloaded": downloaded, "total": total }),
            );
        }
    }
    file.flush().await.map_err(|e| e.to_string())?;

    Ok(())
}

/// Extracts just `lib/libpdfium.dylib` out of a `pdfium-mac-arm64.tgz`
/// release asset into `<dest_root>/lib/libpdfium.dylib`.
fn extract_pdfium_dylib(tgz_path: &Path, dest_root: &Path) -> std::io::Result<()> {
    let file = std::fs::File::open(tgz_path)?;
    let gz = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(gz);

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        if path == Path::new("lib/libpdfium.dylib") {
            let dest_lib_dir = dest_root.join("lib");
            std::fs::create_dir_all(&dest_lib_dir)?;
            entry.unpack(dest_lib_dir.join(PDFIUM_DYLIB_NAME))?;
            return Ok(());
        }
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "lib/libpdfium.dylib not found inside pdfium-mac-arm64.tgz",
    ))
}
