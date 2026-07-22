import { Checkbox } from "@/components/ui/checkbox"
import { Label } from "@/components/ui/label"

export function VerifySection({
  checked,
  onCheckedChange,
  codexStatusLine,
}: {
  checked: boolean
  onCheckedChange: (checked: boolean) => void
  codexStatusLine: string | null
}) {
  return (
    <div className="mb-2">
      <Label className="flex w-fit cursor-pointer items-center gap-2 text-sm font-normal normal-case">
        <Checkbox
          checked={checked}
          onCheckedChange={(next) => onCheckedChange(next === true)}
        />
        Verify crops with Codex (slower)
      </Label>

      {checked && (
        <p className="mt-2 mb-2 text-sm text-muted-foreground">
          Uses the <code>codex</code> CLI to double-check each crop and correct it
          if it's cut off or includes too much extra content, retrying up to 3
          times per object. Requires network access and adds a Codex call per
          object on top of the existing (already CPU/time-heavy) extraction
          pipeline.
        </p>
      )}
      {checked && codexStatusLine && (
        <p className="mb-2 text-sm text-muted-foreground">{codexStatusLine}</p>
      )}
    </div>
  )
}
