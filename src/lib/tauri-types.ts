// Mirrors the Rust types in `src-tauri/src/commands.rs` and
// `src-tauri/src/pipeline/types.rs`. Command return values and the
// manifest are serialized with serde's default (snake_case) casing;
// pipeline events are hand-built with `serde_json::json!` in commands.rs
// and use camelCase keys instead - keep that distinction when adding
// fields, it's intentional on the Rust side, not a typo here.

export type Kind =
  | "figure"
  | "table"
  | "formula"
  | "algorithm"
  | "aside"
  | "seal"

export type OutputFormat = "webp" | "avif" | "png" | "jpeg" | "jpegxl"

export interface PdfInfo {
  path: string
  page_count: number
}

export interface ModelStatus {
  model_present: boolean
  pdfium_present: boolean
  using_dev_assets: boolean
}

export type VerifyBackend = "off" | "codex" | "claude"

export interface BackendStatus {
  available: boolean
  detail: string
}

export interface ExportedFiles {
  format: string
  with_caption: string
  no_caption: string
}

export interface VerificationAttempt {
  attempt: number
  passed: boolean
  issue: string
  reason: string
  bbox_adjustment_pt: [number, number, number, number] | null
}

export interface VerificationInfo {
  enabled: boolean
  attempts: number
  passed: boolean
  last_issue: string | null
  history: VerificationAttempt[]
}

export interface ManifestEntry {
  id: string
  kind: string
  raw_label: string
  page_index: number
  score: number
  bbox_pt: [number, number, number, number]
  with_caption_bbox_pt: [number, number, number, number]
  has_caption: boolean
  files: ExportedFiles
  verification?: VerificationInfo
}

export interface Manifest {
  source_pdf: string
  page_count: number
  objects: ManifestEntry[]
}

// ---- Pipeline event payloads (camelCase - see commands.rs `run_extraction`) ----

export interface ModelDownloadProgressPayload {
  stage: "config" | "model" | "pdfium" | string
  downloaded: number
  total: number
}

export interface PageDetectedPayload {
  jobId: string
  pageIndex: number
  pageCount: number
  countsByKind: Record<string, number>
}

export interface ObjectExportedPayload {
  jobId: string
  id: string
  kind: string
  pageIndex: number
}

export interface ExtractionCompletePayload {
  jobId: string
  manifestPath: string
  objectCount: number
}

export interface ExtractionErrorPayload {
  jobId: string
  message: string
}
