const { invoke, convertFileSrc } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { getCurrentWebview } = window.__TAURI__.webview;

// ---- DOM refs -------------------------------------------------------------

const modelBanner = document.querySelector("#model-banner");
const modelBannerTitle = document.querySelector("#model-banner-title");
const modelBannerDetail = document.querySelector("#model-banner-detail");
const downloadModelBtn = document.querySelector("#download-model-btn");

const downloadProgressSection = document.querySelector("#download-progress");
const downloadStageLabel = document.querySelector("#download-stage-label");
const downloadPercentLabel = document.querySelector("#download-percent-label");
const downloadProgressBar = document.querySelector("#download-progress-bar");

const dropZone = document.querySelector("#drop-zone");
const dropZoneText = document.querySelector("#drop-zone-text");
const choosePdfBtn = document.querySelector("#choose-pdf-btn");
const pdfInfo = document.querySelector("#pdf-info");

const chooseOutputBtn = document.querySelector("#choose-output-btn");
const outputDirLabel = document.querySelector("#output-dir-label");

const extractBtn = document.querySelector("#extract-btn");
const cancelBtn = document.querySelector("#cancel-btn");
const extractDisabledReason = document.querySelector("#extract-disabled-reason");
const progressArea = document.querySelector("#progress-area");
const progressPageLabel = document.querySelector("#progress-page-label");
const countsRow = document.querySelector("#counts-row");
const errorLine = document.querySelector("#error-line");

const resultsSection = document.querySelector("#results-section");
const resultsSummary = document.querySelector("#results-summary");
const resultsGallery = document.querySelector("#results-gallery");

const objectModal = document.querySelector("#object-modal");
const modalBody = document.querySelector("#modal-body");
const modalCloseBtn = document.querySelector("#modal-close-btn");

const KIND_ORDER = ["figure", "table", "formula", "algorithm", "aside", "seal"];

// ---- App state --------------------------------------------------------

let currentPdf = null; // { path, page_count }
let currentOutputDir = null;
let outputDirIsDefaulted = false;
let modelStatus = null;
let currentJobId = null;
let cumulativeCounts = {};

// ---- Small helpers ------------------------------------------------------

function pdfStem(path) {
  const base = path.split("/").pop() || path;
  return base.replace(/\.pdf$/i, "");
}

function dirName(path) {
  const idx = path.lastIndexOf("/");
  return idx >= 0 ? path.slice(0, idx) : ".";
}

function formatBytes(n) {
  if (!n || n <= 0) return "0 MB";
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

function setHidden(el, hidden) {
  el.classList.toggle("hidden", hidden);
}

// ---- Model status ---------------------------------------------------------

async function refreshModelStatus() {
  try {
    modelStatus = await invoke("model_status");
  } catch (e) {
    modelStatus = { model_present: false, pdfium_present: false, using_dev_assets: false };
  }
  const ready = modelStatus.model_present && modelStatus.pdfium_present;
  setHidden(modelBanner, ready);
  if (!ready) {
    const missing = [];
    if (!modelStatus.model_present) missing.push("detection model");
    if (!modelStatus.pdfium_present) missing.push("PDFium library");
    modelBannerTitle.textContent = "Model not ready";
    modelBannerDetail.textContent = `Missing: ${missing.join(" and ")}.`;
  }
  updateExtractButtonState();
  return ready;
}

downloadModelBtn.addEventListener("click", async () => {
  downloadModelBtn.disabled = true;
  setHidden(downloadProgressSection, false);
  downloadStageLabel.textContent = "Starting download…";
  downloadPercentLabel.textContent = "0%";
  downloadProgressBar.style.width = "0%";
  try {
    await invoke("download_model");
    setHidden(downloadProgressSection, true);
    await refreshModelStatus();
  } catch (e) {
    downloadStageLabel.textContent = `Download failed: ${e}`;
  } finally {
    downloadModelBtn.disabled = false;
  }
});

const STAGE_LABELS = { config: "Downloading config…", model: "Downloading detection model…", pdfium: "Downloading PDFium…" };

listen("model-download-progress", (event) => {
  const { stage, downloaded, total } = event.payload;
  downloadStageLabel.textContent = STAGE_LABELS[stage] || `Downloading ${stage}…`;
  if (total > 0) {
    const pct = Math.min(100, Math.round((downloaded / total) * 100));
    downloadPercentLabel.textContent = `${pct}% (${formatBytes(downloaded)} / ${formatBytes(total)})`;
    downloadProgressBar.style.width = `${pct}%`;
  } else {
    downloadPercentLabel.textContent = formatBytes(downloaded);
  }
});

// ---- PDF selection ----------------------------------------------------

async function loadPdf(path) {
  try {
    errorLine.classList.add("hidden");
    const info = await invoke("open_pdf", { path });
    currentPdf = info;
    pdfInfo.textContent = `${path} — ${info.page_count} page${info.page_count === 1 ? "" : "s"}`;
    setHidden(pdfInfo, false);
    dropZoneText.textContent = "Drop another PDF, or";

    if (!currentOutputDir || outputDirIsDefaulted) {
      currentOutputDir = `${dirName(path)}/extracted`;
      outputDirIsDefaulted = true;
      outputDirLabel.textContent = `${currentOutputDir} (default — click to change)`;
    }
    setHidden(resultsSection, true);
    updateExtractButtonState();
  } catch (e) {
    currentPdf = null;
    pdfInfo.textContent = String(e);
    setHidden(pdfInfo, false);
    updateExtractButtonState();
  }
}

choosePdfBtn.addEventListener("click", async () => {
  if (currentJobId) return; // PDFium is in use by the running extraction
  const picked = await invoke("pick_pdf_file");
  if (picked) await loadPdf(picked);
});

getCurrentWebview().onDragDropEvent((event) => {
  if (event.payload.type !== "drop") return;
  if (currentJobId) return; // PDFium is in use by the running extraction
  const pdfPath = event.payload.paths.find((p) => p.toLowerCase().endsWith(".pdf"));
  if (pdfPath) {
    loadPdf(pdfPath);
  } else {
    pdfInfo.textContent = "That doesn't look like a PDF file.";
    setHidden(pdfInfo, false);
  }
});

// ---- Output dir selection -----------------------------------------------

chooseOutputBtn.addEventListener("click", async () => {
  const picked = await invoke("pick_output_dir");
  if (picked) {
    currentOutputDir = picked;
    outputDirIsDefaulted = false;
    outputDirLabel.textContent = picked;
    updateExtractButtonState();
  }
});

// ---- Extract button gating ------------------------------------------------

function updateExtractButtonState() {
  const modelReady = modelStatus && modelStatus.model_present && modelStatus.pdfium_present;
  const reasons = [];
  if (!currentPdf) reasons.push("choose a PDF");
  if (!currentOutputDir) reasons.push("choose an output folder");
  if (!modelReady) reasons.push("model not ready — download it above");

  const busy = currentJobId !== null;
  extractBtn.disabled = reasons.length > 0 || busy;
  extractDisabledReason.textContent = busy ? "" : reasons.length ? `Waiting on: ${reasons.join(", ")}` : "";

  // PDFium is single-instance/serialized on the Rust side, so choosing a
  // different PDF while an extraction is using it isn't safe.
  choosePdfBtn.disabled = busy;
  dropZone.classList.toggle("busy", busy);
}

// ---- Extraction lifecycle -----------------------------------------------

extractBtn.addEventListener("click", async () => {
  if (!currentPdf || !currentOutputDir) return;
  errorLine.classList.add("hidden");
  setHidden(resultsSection, true);
  cumulativeCounts = {};
  renderCounts();
  setHidden(progressArea, false);
  setHidden(cancelBtn, false);
  progressPageLabel.textContent = "Starting extraction…";

  try {
    currentJobId = await invoke("run_extraction", {
      pdfPath: currentPdf.path,
      outputDir: currentOutputDir,
    });
  } catch (e) {
    showError(String(e));
    return;
  }
  updateExtractButtonState();
});

cancelBtn.addEventListener("click", async () => {
  if (!currentJobId) return;
  cancelBtn.disabled = true;
  progressPageLabel.textContent = "Cancelling… (finishing current page)";
  try {
    await invoke("cancel_extraction", { jobId: currentJobId });
  } catch (e) {
    // job may have already finished
  }
});

function showError(message) {
  errorLine.textContent = message;
  setHidden(errorLine, false);
  setHidden(progressArea, true);
  setHidden(cancelBtn, true);
  currentJobId = null;
  cancelBtn.disabled = false;
  updateExtractButtonState();
}

function renderCounts() {
  countsRow.innerHTML = "";
  const kinds = Object.keys(cumulativeCounts).length
    ? KIND_ORDER.filter((k) => cumulativeCounts[k])
    : [];
  for (const kind of kinds) {
    const badge = document.createElement("span");
    badge.className = `count-badge kind-${kind}`;
    badge.textContent = `${kind}: ${cumulativeCounts[kind]}`;
    countsRow.appendChild(badge);
  }
}

listen("page-detected", (event) => {
  const { jobId, pageIndex, pageCount } = event.payload;
  if (jobId !== currentJobId) return;
  progressPageLabel.textContent = `Processing page ${pageIndex + 1} of ${pageCount}…`;
});

listen("object-exported", (event) => {
  const { jobId, kind } = event.payload;
  if (jobId !== currentJobId) return;
  cumulativeCounts[kind] = (cumulativeCounts[kind] || 0) + 1;
  renderCounts();
});

listen("extraction-complete", async (event) => {
  const { jobId, objectCount } = event.payload;
  if (jobId !== currentJobId) return;
  currentJobId = null;
  setHidden(progressArea, true);
  setHidden(cancelBtn, true);
  cancelBtn.disabled = false;
  updateExtractButtonState();

  resultsSummary.textContent = `${objectCount} object${objectCount === 1 ? "" : "s"} extracted.`;
  setHidden(resultsSection, false);

  try {
    const manifest = await invoke("list_results", {
      outputDir: currentOutputDir,
      pdfStem: pdfStem(currentPdf.path),
    });
    renderGallery(manifest);
  } catch (e) {
    resultsSummary.textContent += ` (failed to load gallery: ${e})`;
  }
});

listen("extraction-error", (event) => {
  const { jobId, message } = event.payload;
  if (jobId !== currentJobId) return;
  showError(message);
});

// ---- Results gallery --------------------------------------------------

function renderGallery(manifest) {
  resultsGallery.innerHTML = "";
  const byPage = new Map();
  for (const entry of manifest.objects) {
    if (!byPage.has(entry.page_index)) byPage.set(entry.page_index, []);
    byPage.get(entry.page_index).push(entry);
  }
  const pageIndices = [...byPage.keys()].sort((a, b) => a - b);

  if (pageIndices.length === 0) {
    resultsGallery.innerHTML = "<p class='info-line'>No figures/tables/formulas/algorithm blocks were detected.</p>";
    return;
  }

  for (const pageIndex of pageIndices) {
    const pageBlock = document.createElement("div");
    pageBlock.className = "page-block";
    const heading = document.createElement("h3");
    heading.textContent = `Page ${pageIndex + 1}`;
    pageBlock.appendChild(heading);

    const grid = document.createElement("div");
    grid.className = "thumb-grid";
    for (const entry of byPage.get(pageIndex)) {
      grid.appendChild(renderThumb(entry));
    }
    pageBlock.appendChild(grid);
    resultsGallery.appendChild(pageBlock);
  }
}

function renderThumb(entry) {
  const card = document.createElement("button");
  card.type = "button";
  card.className = `thumb-card kind-${entry.kind}`;

  const img = document.createElement("img");
  img.src = convertFileSrc(entry.files.with_caption_webp);
  img.alt = `${entry.kind} on page ${entry.page_index + 1}`;
  card.appendChild(img);

  const label = document.createElement("div");
  label.className = "thumb-label";
  label.textContent = `${entry.kind} · ${entry.id}`;
  card.appendChild(label);

  card.addEventListener("click", () => openObjectModal(entry));
  return card;
}

function openObjectModal(entry) {
  modalBody.innerHTML = "";

  const img = document.createElement("img");
  img.className = "modal-image";
  img.src = convertFileSrc(entry.files.with_caption_webp);
  modalBody.appendChild(img);

  const title = document.createElement("h3");
  title.textContent = `${entry.kind} — page ${entry.page_index + 1} (score ${(entry.score * 100).toFixed(0)}%)`;
  modalBody.appendChild(title);

  if (!entry.has_caption) {
    const note = document.createElement("p");
    note.className = "info-line";
    note.textContent = "No caption/number box was found nearby — the with/without-caption crops are identical.";
    modalBody.appendChild(note);
  }

  const fileList = document.createElement("div");
  fileList.className = "file-list";
  const files = [
    ["With caption · WebP", entry.files.with_caption_webp],
    ["No caption · WebP", entry.files.no_caption_webp],
    ["With caption · AVIF", entry.files.with_caption_avif],
    ["No caption · AVIF", entry.files.no_caption_avif],
  ];
  for (const [label, path] of files) {
    const row = document.createElement("div");
    row.className = "file-row";

    const text = document.createElement("span");
    text.className = "file-path";
    text.textContent = `${label}: ${path}`;
    row.appendChild(text);

    const revealBtn = document.createElement("button");
    revealBtn.type = "button";
    revealBtn.textContent = "Reveal in Finder";
    revealBtn.addEventListener("click", () => invoke("reveal_in_finder", { path }));
    row.appendChild(revealBtn);

    fileList.appendChild(row);
  }
  modalBody.appendChild(fileList);

  setHidden(objectModal, false);
}

modalCloseBtn.addEventListener("click", () => setHidden(objectModal, true));
objectModal.querySelector(".modal-backdrop").addEventListener("click", () => setHidden(objectModal, true));

// ---- Boot -----------------------------------------------------------------

window.addEventListener("DOMContentLoaded", () => {
  refreshModelStatus();
});
