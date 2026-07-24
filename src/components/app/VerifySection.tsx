import { Label } from "@/components/ui/label"
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group"
import type { VerifyBackend } from "@/lib/tauri-types"

const BACKENDS: { value: VerifyBackend; label: string }[] = [
  { value: "off", label: "Off" },
  { value: "codex", label: "Codex CLI" },
  { value: "claude", label: "Claude Code" },
]

export function VerifySection({
  value,
  onValueChange,
  statusLine,
}: {
  value: VerifyBackend
  onValueChange: (value: VerifyBackend) => void
  statusLine: string | null
}) {
  return (
    <div className="mb-2">
      <RadioGroup
        value={value}
        onValueChange={(next) => onValueChange(next as VerifyBackend)}
        className="flex w-auto flex-wrap gap-4"
      >
        {BACKENDS.map((backend) => (
          <Label
            key={backend.value}
            className="flex cursor-pointer items-center gap-1.5 text-sm font-normal normal-case"
          >
            <RadioGroupItem value={backend.value} />
            {backend.label}
          </Label>
        ))}
      </RadioGroup>

      {value !== "off" && (
        <p className="mt-2 mb-2 text-sm text-muted-foreground">
          Uses the <code>{value}</code> CLI to double-check each crop and
          correct it if it's cut off or includes too much extra content,
          retrying up to 3 times per object. Requires network access and adds an
          AI call per object on top of the existing (already CPU/time- heavy)
          extraction pipeline.
        </p>
      )}
      {value !== "off" && statusLine && (
        <p className="mb-2 text-sm text-muted-foreground">{statusLine}</p>
      )}
    </div>
  )
}
