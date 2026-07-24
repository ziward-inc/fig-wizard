import { listen } from "@tauri-apps/api/event"
import { getCurrentWebview } from "@tauri-apps/api/webview"
import { useCallback, useEffect, useRef, useState } from "react"
import { ExtractSection } from "@/components/app/ExtractSection"
import { FormatPicker } from "@/components/app/FormatPicker"
import { type DownloadState, ModelBanner } from "@/components/app/ModelBanner"
import { PdfDropZone } from "@/components/app/PdfDropZone"
import { ResultsGallery } from "@/components/app/ResultsGallery"
import { VerifySection } from "@/components/app/VerifySection"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { dirName, pdfStem } from "@/lib/format"
import {
  backendStatus,
  cancelExtraction,
  downloadModel,
  modelStatus as fetchModelStatus,
  listResults,
  openPdf,
  pickOutputDir,
  pickPdfFile,
  runExtraction,
} from "@/lib/tauri-commands"
import type {
  ExtractionCompletePayload,
  ExtractionErrorPayload,
  Manifest,
  ModelDownloadProgressPayload,
  ModelStatus,
  ObjectExportedPayload,
  OutputFormat,
  PageDetectedPayload,
  PdfInfo,
  VerifyBackend,
} from "@/lib/tauri-types"

const BACKEND_LABELS: Record<VerifyBackend, string> = {
  off: "",
  codex: "Codex CLI",
  claude: "Claude Code",
}

export function App() {
  const [currentPdf, setCurrentPdfState] = useState<PdfInfo | null>(null)
  const [pdfError, setPdfError] = useState<string | null>(null)
  const [currentOutputDir, setCurrentOutputDirState] = useState<string | null>(
    null
  )
  const [outputDirIsDefaulted, setOutputDirIsDefaulted] = useState(false)

  const [modelStatus, setModelStatus] = useState<ModelStatus | null>(null)
  const [downloadState, setDownloadState] = useState<DownloadState>("idle")
  const [downloadProgress, setDownloadProgress] =
    useState<ModelDownloadProgressPayload | null>(null)
  const [downloadError, setDownloadError] = useState<string | null>(null)

  const [outputFormat, setOutputFormat] = useState<OutputFormat>("webp")

  const [verifyBackend, setVerifyBackend] = useState<VerifyBackend>("off")
  const [backendAvailable, setBackendAvailable] = useState<boolean | null>(
    null
  )
  const [backendStatusLine, setBackendStatusLine] = useState<string | null>(
    null
  )

  const [currentJobId, setCurrentJobIdState] = useState<string | null>(null)
  const [cancelling, setCancelling] = useState(false)
  const [progressLabel, setProgressLabel] = useState("Starting extraction…")
  const [cumulativeCounts, setCumulativeCounts] = useState<
    Record<string, number>
  >({})
  const [extractionError, setExtractionError] = useState<string | null>(null)

  const [resultsSummary, setResultsSummary] = useState<string | null>(null)
  const [resultsManifest, setResultsManifest] = useState<Manifest | null>(null)
  const [resultsDir, setResultsDir] = useState<string | null>(null)

  // Refs mirror the state above so the long-lived Tauri event listeners
  // (set up once on mount) always see the latest value instead of closing
  // over the value from whichever render registered them.
  const currentJobIdRef = useRef<string | null>(null)
  const currentPdfRef = useRef<PdfInfo | null>(null)
  const currentOutputDirRef = useRef<string | null>(null)

  const setCurrentJobId = useCallback((id: string | null) => {
    currentJobIdRef.current = id
    setCurrentJobIdState(id)
  }, [])
  const setCurrentPdf = useCallback((info: PdfInfo | null) => {
    currentPdfRef.current = info
    setCurrentPdfState(info)
  }, [])
  const setCurrentOutputDir = useCallback((dir: string | null) => {
    currentOutputDirRef.current = dir
    setCurrentOutputDirState(dir)
  }, [])

  const refreshModelStatus = useCallback(async () => {
    try {
      const status = await fetchModelStatus()
      setModelStatus(status)
    } catch {
      setModelStatus({
        model_present: false,
        pdfium_present: false,
        using_dev_assets: false,
      })
    }
  }, [])

  useEffect(() => {
    refreshModelStatus()
  }, [refreshModelStatus])

  const loadPdf = useCallback(
    async (path: string) => {
      setPdfError(null)
      try {
        const info = await openPdf(path)
        setCurrentPdf(info)
        if (!currentOutputDirRef.current || outputDirIsDefaulted) {
          setCurrentOutputDir(`${dirName(path)}/extracted`)
          setOutputDirIsDefaulted(true)
        }
        setResultsManifest(null)
        setResultsSummary(null)
        setResultsDir(null)
      } catch (e) {
        setCurrentPdf(null)
        setPdfError(String(e))
      }
    },
    [outputDirIsDefaulted, setCurrentOutputDir, setCurrentPdf]
  )

  useEffect(() => {
    let unlistenDragDrop: (() => void) | undefined
    getCurrentWebview()
      .onDragDropEvent((event) => {
        if (event.payload.type !== "drop") return
        if (currentJobIdRef.current) return // PDFium is in use by the running extraction
        const path = event.payload.paths.find((p) =>
          p.toLowerCase().endsWith(".pdf")
        )
        if (path) {
          loadPdf(path)
        } else {
          setCurrentPdf(null)
          setPdfError("That doesn't look like a PDF file.")
        }
      })
      .then((fn) => {
        unlistenDragDrop = fn
      })
    return () => unlistenDragDrop?.()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [setCurrentPdf, loadPdf])

  useEffect(() => {
    const unlistenPromises = [
      listen<ModelDownloadProgressPayload>(
        "model-download-progress",
        (event) => {
          setDownloadProgress(event.payload)
        }
      ),
      listen<PageDetectedPayload>("page-detected", (event) => {
        if (event.payload.jobId !== currentJobIdRef.current) return
        setProgressLabel(
          `Processing page ${event.payload.pageIndex + 1} of ${event.payload.pageCount}…`
        )
      }),
      listen<ObjectExportedPayload>("object-exported", (event) => {
        if (event.payload.jobId !== currentJobIdRef.current) return
        setCumulativeCounts((prev) => ({
          ...prev,
          [event.payload.kind]: (prev[event.payload.kind] ?? 0) + 1,
        }))
      }),
      listen<ExtractionCompletePayload>(
        "extraction-complete",
        async (event) => {
          if (event.payload.jobId !== currentJobIdRef.current) return
          setCurrentJobId(null)
          setCancelling(false)
          const count = event.payload.objectCount
          setResultsSummary(
            `${count} object${count === 1 ? "" : "s"} extracted.`
          )
          setResultsDir(dirName(event.payload.manifestPath))
          const outputDir = currentOutputDirRef.current
          const currentPdf = currentPdfRef.current
          if (outputDir && currentPdf) {
            try {
              const manifest = await listResults({
                outputDir,
                pdfStem: pdfStem(currentPdf.path),
              })
              setResultsManifest(manifest)
            } catch (e) {
              setResultsSummary(
                (prev) => `${prev} (failed to load results: ${e})`
              )
            }
          }
        }
      ),
      listen<ExtractionErrorPayload>("extraction-error", (event) => {
        if (event.payload.jobId !== currentJobIdRef.current) return
        setExtractionError(event.payload.message)
        setCurrentJobId(null)
        setCancelling(false)
      }),
    ]
    return () => {
      unlistenPromises.forEach((p) => {
        void p.then((fn) => fn())
      })
    }
  }, [setCurrentJobId])

  const handleDownloadModel = useCallback(async () => {
    setDownloadState("downloading")
    setDownloadProgress(null)
    setDownloadError(null)
    try {
      await downloadModel()
      setDownloadState("idle")
      await refreshModelStatus()
    } catch (e) {
      setDownloadState("error")
      setDownloadError(String(e))
    }
  }, [refreshModelStatus])

  const handleChoosePdf = useCallback(async () => {
    if (currentJobIdRef.current) return // PDFium is in use by the running extraction
    const picked = await pickPdfFile()
    if (picked) await loadPdf(picked)
  }, [loadPdf])

  const handleChooseOutputDir = useCallback(async () => {
    const picked = await pickOutputDir()
    if (picked) {
      setCurrentOutputDir(picked)
      setOutputDirIsDefaulted(false)
    }
  }, [setCurrentOutputDir])

  const handleVerifyBackendChange = useCallback(
    async (backend: VerifyBackend) => {
      setVerifyBackend(backend)
      if (backend === "off") {
        setBackendStatusLine(null)
        setBackendAvailable(null)
        return
      }
      const label = BACKEND_LABELS[backend]
      setBackendStatusLine(`Checking for ${label}…`)
      try {
        const status = await backendStatus(backend)
        setBackendAvailable(status.available)
        setBackendStatusLine(
          status.available
            ? `${label} found (${status.detail}). Verification will run per object.`
            : `${label} not available: ${status.detail}. Choose "Off", or install/authenticate ${label} first.`
        )
      } catch (e) {
        setBackendAvailable(false)
        setBackendStatusLine(`Could not check ${label} status: ${e}`)
      }
    },
    []
  )

  const handleExtract = useCallback(async () => {
    if (!currentPdf || !currentOutputDir) return
    setExtractionError(null)
    setResultsManifest(null)
    setResultsSummary(null)
    setResultsDir(null)
    setCumulativeCounts({})
    setProgressLabel("Starting extraction…")
    try {
      const jobId = await runExtraction({
        pdfPath: currentPdf.path,
        outputDir: currentOutputDir,
        outputFormat,
        verifyBackend,
      })
      setCurrentJobId(jobId)
    } catch (e) {
      setExtractionError(String(e))
    }
  }, [
    currentOutputDir,
    currentPdf,
    outputFormat,
    setCurrentJobId,
    verifyBackend,
  ])

  const handleCancel = useCallback(async () => {
    if (!currentJobId) return
    setCancelling(true)
    setProgressLabel("Cancelling… (finishing current page)")
    try {
      await cancelExtraction(currentJobId)
    } catch {
      // job may have already finished
    }
  }, [currentJobId])

  const modelReady = !!(
    modelStatus?.model_present && modelStatus?.pdfium_present
  )
  const busy = currentJobId !== null
  const extractReasons: string[] = []
  if (!currentPdf) extractReasons.push("choose a PDF")
  if (!currentOutputDir) extractReasons.push("choose an output folder")
  if (!modelReady) extractReasons.push("model not ready — download it above")
  if (verifyBackend !== "off" && backendAvailable === false)
    extractReasons.push(`${BACKEND_LABELS[verifyBackend]} not available`)
  const extractDisabled = extractReasons.length > 0 || busy
  const disabledReason = extractReasons.length
    ? `Waiting on: ${extractReasons.join(", ")}`
    : ""

  return (
    <main className="mx-auto max-w-3xl px-6 py-8 pb-16">
      <header className="mb-6">
        <h1 className="mb-1 text-2xl font-semibold normal-case">
          PDF Paper Image Extractor
        </h1>
        <p className="text-muted-foreground">
          Pull figures, tables, formulas, and algorithm blocks out of an
          academic paper PDF.
        </p>
      </header>

      <ModelBanner
        modelStatus={modelStatus}
        downloadState={downloadState}
        downloadProgress={downloadProgress}
        downloadError={downloadError}
        onDownload={handleDownloadModel}
      />

      <Card className="mb-5">
        <CardHeader>
          <CardTitle>1. Choose a PDF</CardTitle>
        </CardHeader>
        <CardContent>
          <PdfDropZone
            currentPdf={currentPdf}
            pdfError={pdfError}
            busy={busy}
            onChoosePdf={handleChoosePdf}
          />
        </CardContent>
      </Card>

      <Card className="mb-5">
        <CardHeader>
          <CardTitle>2. Choose an output folder</CardTitle>
        </CardHeader>
        <CardContent>
          <div className="flex flex-wrap items-center gap-3">
            <Button onClick={handleChooseOutputDir}>
              Choose output folder…
            </Button>
            <span className="text-sm text-muted-foreground">
              {currentOutputDir
                ? outputDirIsDefaulted
                  ? `${currentOutputDir} (default — click to change)`
                  : currentOutputDir
                : "No folder selected yet"}
            </span>
          </div>
        </CardContent>
      </Card>

      <Card className="mb-5">
        <CardHeader>
          <CardTitle>3. Choose an output format</CardTitle>
        </CardHeader>
        <CardContent>
          <FormatPicker
            value={outputFormat}
            onValueChange={setOutputFormat}
            busy={busy}
          />
        </CardContent>
      </Card>

      <Card className="mb-5">
        <CardHeader>
          <CardTitle>4. Extract</CardTitle>
        </CardHeader>
        <CardContent>
          <VerifySection
            value={verifyBackend}
            onValueChange={handleVerifyBackendChange}
            statusLine={backendStatusLine}
          />
          <ExtractSection
            onExtract={handleExtract}
            onCancel={handleCancel}
            extractDisabled={extractDisabled}
            disabledReason={disabledReason}
            busy={busy}
            cancelling={cancelling}
            progressLabel={progressLabel}
            counts={cumulativeCounts}
            error={extractionError}
          />
        </CardContent>
      </Card>

      {resultsManifest && resultsDir && (
        <Card>
          <CardHeader>
            <CardTitle>Results</CardTitle>
          </CardHeader>
          <CardContent>
            <ResultsGallery
              manifest={resultsManifest}
              resultDir={resultsDir}
              summary={resultsSummary}
            />
          </CardContent>
        </Card>
      )}
    </main>
  )
}

export default App
