import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { Button } from "@/components/ui/button"
import { Progress } from "@/components/ui/progress"
import { formatBytes } from "@/lib/format"
import type {
  ModelDownloadProgressPayload,
  ModelStatus,
} from "@/lib/tauri-types"

const STAGE_LABELS: Record<string, string> = {
  config: "Downloading config…",
  model: "Downloading detection model…",
}

export type DownloadState = "idle" | "downloading" | "error"

export function ModelBanner({
  modelStatus,
  downloadState,
  downloadProgress,
  downloadError,
  onDownload,
}: {
  modelStatus: ModelStatus | null
  downloadState: DownloadState
  downloadProgress: ModelDownloadProgressPayload | null
  downloadError: string | null
  onDownload: () => void
}) {
  const ready = modelStatus?.model_present && modelStatus?.pdfium_present
  if (ready) return null

  const missing: string[] = []
  if (!modelStatus?.model_present) missing.push("detection model")
  if (!modelStatus?.pdfium_present) missing.push("PDFium library")

  const pct =
    downloadProgress && downloadProgress.total > 0
      ? Math.min(
          100,
          Math.round(
            (downloadProgress.downloaded / downloadProgress.total) * 100
          )
        )
      : null

  return (
    <Alert className="mb-5 border-amber-300 bg-amber-50 dark:border-amber-900 dark:bg-amber-950">
      <div className="flex flex-wrap items-center justify-between gap-4">
        <div className="flex flex-col gap-1">
          <AlertTitle className="text-amber-900 dark:text-amber-200">
            Model not ready
          </AlertTitle>
          <AlertDescription className="text-amber-800 dark:text-amber-300">
            Missing: {missing.join(" and ")}.
          </AlertDescription>
        </div>
        <Button
          size="sm"
          onClick={onDownload}
          disabled={downloadState === "downloading"}
        >
          Download model (~125MB)
        </Button>
      </div>

      {downloadState !== "idle" && (
        <div className="mt-3">
          {downloadState === "error" ? (
            <p className="text-sm text-destructive">
              Download failed: {downloadError}
            </p>
          ) : (
            <>
              <div className="mb-1 flex justify-between text-sm text-muted-foreground">
                <span>
                  {STAGE_LABELS[downloadProgress?.stage ?? ""] ??
                    (downloadProgress
                      ? `Downloading ${downloadProgress.stage}…`
                      : "Starting download…")}
                </span>
                <span>
                  {downloadProgress && pct !== null
                    ? `${pct}% (${formatBytes(downloadProgress.downloaded)} / ${formatBytes(downloadProgress.total)})`
                    : downloadProgress
                      ? formatBytes(downloadProgress.downloaded)
                      : "0%"}
                </span>
              </div>
              <Progress value={pct} />
            </>
          )}
        </div>
      )}
    </Alert>
  )
}
