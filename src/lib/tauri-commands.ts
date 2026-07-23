import { invoke } from "@tauri-apps/api/core"

import type {
  CodexStatus,
  Manifest,
  ModelStatus,
  OutputFormat,
  PdfInfo,
} from "@/lib/tauri-types"

export const openPdf = (path: string) => invoke<PdfInfo>("open_pdf", { path })

export const pickPdfFile = () => invoke<string | null>("pick_pdf_file")

export const pickOutputDir = () => invoke<string | null>("pick_output_dir")

export const modelStatus = () => invoke<ModelStatus>("model_status")

export const downloadModel = () => invoke<void>("download_model")

export const runExtraction = (args: {
  pdfPath: string
  outputDir: string
  outputFormat: OutputFormat
  verifyWithCodex: boolean
}) => invoke<string>("run_extraction", args)

export const cancelExtraction = (jobId: string) =>
  invoke<void>("cancel_extraction", { jobId })

export const listResults = (args: { outputDir: string; pdfStem: string }) =>
  invoke<Manifest>("list_results", args)

export const revealInFinder = (path: string) =>
  invoke<void>("reveal_in_finder", { path })

export const codexStatus = () => invoke<CodexStatus>("codex_status")
