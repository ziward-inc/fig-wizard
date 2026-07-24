import { Badge } from "@/components/ui/badge"
import type { VerificationInfo } from "@/lib/tauri-types"
import { cn } from "@/lib/utils"

export function VerificationBadge({
  verification,
  className,
}: {
  verification: VerificationInfo | undefined
  className?: string
}) {
  if (!verification?.enabled) return null

  const tries = `${verification.attempts} ${verification.attempts === 1 ? "try" : "tries"}`

  if (verification.passed && verification.attempts === 1) {
    return (
      <Badge
        className={cn("bg-emerald-100 text-emerald-800", className)}
        title="AI verified this crop on the first attempt."
      >
        ✓ 1 try
      </Badge>
    )
  }
  if (verification.passed) {
    return (
      <Badge
        className={cn("bg-amber-100 text-amber-800", className)}
        title={`AI flagged and corrected this crop (${tries}) before it passed.`}
      >
        ⟳ {tries}
      </Badge>
    )
  }
  return (
    <Badge
      className={cn("bg-red-100 text-red-800", className)}
      title={`AI could not verify this crop as complete after ${tries}${
        verification.last_issue
          ? ` (last issue: ${verification.last_issue})`
          : ""
      }.`}
    >
      ⚠ {tries}, still flagged
    </Badge>
  )
}
