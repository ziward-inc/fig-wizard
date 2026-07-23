import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible"
import type { VerificationInfo } from "@/lib/tauri-types"

export function VerificationHistory({
  verification,
}: {
  verification: VerificationInfo | undefined
}) {
  if (!verification || !verification.enabled || verification.history.length === 0) {
    return null
  }

  return (
    <Collapsible className="mb-3 text-sm">
      <CollapsibleTrigger className="cursor-pointer font-medium text-primary underline underline-offset-2">
        Attempt history ({verification.history.length})
      </CollapsibleTrigger>
      <CollapsibleContent>
        <ul className="mt-2 list-disc pl-5 text-muted-foreground">
          {verification.history.map((attempt) => (
            <li key={attempt.attempt} className="mb-1">
              Attempt {attempt.attempt}: {attempt.passed ? "passed" : attempt.issue}
              {attempt.reason ? ` - ${attempt.reason}` : ""}
            </li>
          ))}
        </ul>
      </CollapsibleContent>
    </Collapsible>
  )
}
