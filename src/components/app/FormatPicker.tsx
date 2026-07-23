import { Label } from "@/components/ui/label"
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group"
import { cn } from "@/lib/utils"
import type { OutputFormat } from "@/lib/tauri-types"

const FORMATS: { value: OutputFormat; label: string }[] = [
  { value: "webp", label: "WebP" },
  { value: "avif", label: "AVIF" },
  { value: "png", label: "PNG" },
  { value: "jpeg", label: "JPEG" },
]

export function FormatPicker({
  value,
  onValueChange,
  busy,
  jpegxlDisabled,
  jpegxlTooltip,
}: {
  value: OutputFormat
  onValueChange: (value: OutputFormat) => void
  busy: boolean
  jpegxlDisabled: boolean
  jpegxlTooltip: string
}) {
  return (
    <>
      <RadioGroup
        value={value}
        onValueChange={(next) => onValueChange(next as OutputFormat)}
        className="mb-3 flex w-auto flex-wrap gap-4"
      >
        {FORMATS.map((format) => (
          <Label
            key={format.value}
            className="flex cursor-pointer items-center gap-1.5 text-sm font-normal normal-case"
          >
            <RadioGroupItem value={format.value} disabled={busy} />
            {format.label}
          </Label>
        ))}

        <Label
          title={jpegxlTooltip}
          className={cn(
            "flex items-center gap-1.5 text-sm font-normal normal-case",
            jpegxlDisabled ? "cursor-not-allowed opacity-55" : "cursor-pointer"
          )}
        >
          <RadioGroupItem value="jpegxl" disabled={jpegxlDisabled} />
          JPEG XL{" "}
          {jpegxlDisabled && (
            <span className="text-xs text-muted-foreground">
              (requires `brew install jpeg-xl`)
            </span>
          )}
        </Label>
      </RadioGroup>
      <p className="text-sm text-muted-foreground">
        Every crop is exported as exactly one format (with-caption and no-caption
        files, quality 85 for lossy formats; PNG is lossless). JPEG XL is encoded
        via a local <code>cjxl</code> subprocess (libjxl) rather than a bundled
        encoder - see README.
      </p>
    </>
  )
}
