import { Alert, AlertDescription } from "@/components/ui/alert"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { KIND_ORDER } from "@/lib/format"

export function ExtractSection({
  onExtract,
  onCancel,
  extractDisabled,
  disabledReason,
  busy,
  cancelling,
  progressLabel,
  counts,
  error,
}: {
  onExtract: () => void
  onCancel: () => void
  extractDisabled: boolean
  disabledReason: string
  busy: boolean
  cancelling: boolean
  progressLabel: string
  counts: Record<string, number>
  error: string | null
}) {
  return (
    <>
      <div className="flex flex-wrap items-center gap-3">
        <Button onClick={onExtract} disabled={extractDisabled}>
          Extract
        </Button>
        {busy && (
          <Button
            variant="destructive"
            onClick={onCancel}
            disabled={cancelling}
          >
            Cancel
          </Button>
        )}
      </div>
      {!busy && disabledReason && (
        <p className="mt-2 text-sm text-muted-foreground">{disabledReason}</p>
      )}

      {busy && (
        <div className="mt-4 flex items-center gap-4 rounded-lg bg-primary/5 p-4">
          <div className="size-7 shrink-0 animate-spin rounded-full border-[3px] border-primary/25 border-t-primary" />
          <div className="flex-1">
            <p className="mb-2 font-medium">{progressLabel}</p>
            <div className="flex flex-wrap gap-2">
              {KIND_ORDER.filter((kind) => counts[kind]).map((kind) => (
                <Badge key={kind} variant="secondary" className="normal-case">
                  {kind}: {counts[kind]}
                </Badge>
              ))}
            </div>
          </div>
        </div>
      )}

      {error && (
        <Alert variant="destructive" className="mt-3">
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      )}
    </>
  )
}
