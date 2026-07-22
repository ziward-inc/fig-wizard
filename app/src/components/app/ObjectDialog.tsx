import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Button } from "@/components/ui/button"
import { ImageWithFallback } from "@/components/app/ImageWithFallback"
import { VerificationBadge } from "@/components/app/VerificationBadge"
import { VerificationHistory } from "@/components/app/VerificationHistory"
import { formatLabel } from "@/lib/format"
import { revealInFinder } from "@/lib/tauri-commands"
import type { ManifestEntry } from "@/lib/tauri-types"

export function ObjectDialog({
  entry,
  onOpenChange,
}: {
  entry: ManifestEntry | null
  onOpenChange: (open: boolean) => void
}) {
  return (
    <Dialog open={entry !== null} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[85vh] max-w-2xl overflow-y-auto">
        {entry && (
          <>
            <ImageWithFallback
              path={entry.files.with_caption}
              alt={`${entry.kind} on page ${entry.page_index + 1}`}
              className="mx-auto h-auto max-h-[45vh] rounded-md border object-contain"
            />
            <DialogHeader>
              <DialogTitle className="normal-case">
                {entry.kind} — page {entry.page_index + 1} (score{" "}
                {(entry.score * 100).toFixed(0)}%)
              </DialogTitle>
            </DialogHeader>

            <VerificationBadge verification={entry.verification} />
            <VerificationHistory verification={entry.verification} />

            {!entry.has_caption && (
              <p className="text-sm text-muted-foreground">
                No caption/number box was found nearby — the with/without-caption
                crops are identical.
              </p>
            )}

            <div className="flex flex-col gap-2">
              {(
                [
                  ["With caption", entry.files.with_caption],
                  ["No caption", entry.files.no_caption],
                ] as const
              ).map(([label, path]) => (
                <div
                  key={path}
                  className="flex items-center justify-between gap-3 rounded-md bg-muted px-3 py-2 text-xs"
                >
                  <span className="break-all text-muted-foreground">
                    {label} · {formatLabel(entry.files.format)}: {path}
                  </span>
                  <Button
                    size="sm"
                    variant="outline"
                    className="shrink-0"
                    onClick={() => revealInFinder(path)}
                  >
                    Reveal in Finder
                  </Button>
                </div>
              ))}
            </div>
          </>
        )}
      </DialogContent>
    </Dialog>
  )
}
