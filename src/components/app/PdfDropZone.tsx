import { Button } from "@/components/ui/button"
import type { PdfInfo } from "@/lib/tauri-types"
import { cn } from "@/lib/utils"

export function PdfDropZone({
  currentPdf,
  pdfError,
  busy,
  onChoosePdf,
}: {
  currentPdf: PdfInfo | null
  pdfError: string | null
  busy: boolean
  onChoosePdf: () => void
}) {
  return (
    <>
      <div
        className={cn(
          "rounded-lg border-2 border-dashed p-8 text-center transition-colors hover:border-primary hover:bg-primary/5",
          busy && "opacity-50"
        )}
      >
        <p className="mb-3">
          {currentPdf ? "Drop another PDF, or" : "Drag & drop a PDF here, or"}
        </p>
        <Button type="button" onClick={onChoosePdf} disabled={busy}>
          Choose PDF…
        </Button>
      </div>
      {currentPdf && !pdfError && (
        <p className="mt-2 text-sm break-all text-muted-foreground">
          {currentPdf.path} — {currentPdf.page_count} page
          {currentPdf.page_count === 1 ? "" : "s"}
        </p>
      )}
      {pdfError && (
        <p className="mt-2 text-sm break-all text-muted-foreground">
          {pdfError}
        </p>
      )}
    </>
  )
}
