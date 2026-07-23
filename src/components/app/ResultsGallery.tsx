import { useMemo } from "react"

import { VerificationBadge } from "@/components/app/VerificationBadge"
import { Button } from "@/components/ui/button"
import { openResultDir } from "@/lib/tauri-commands"
import type { Manifest, ManifestEntry } from "@/lib/tauri-types"
import { cn } from "@/lib/utils"

const KIND_LABEL_COLOR: Record<string, string> = {
  figure: "text-blue-700 dark:text-blue-400",
  table: "text-green-700 dark:text-green-400",
  formula: "text-amber-700 dark:text-amber-400",
  algorithm: "text-purple-700 dark:text-purple-400",
  aside: "text-slate-500 dark:text-slate-400",
}

export function ResultsGallery({
  manifest,
  resultDir,
  summary,
}: {
  manifest: Manifest
  resultDir: string
  summary: string | null
}) {
  const byPage = useMemo(() => {
    const map = new Map<number, ManifestEntry[]>()
    for (const entry of manifest.objects) {
      const list = map.get(entry.page_index) ?? []
      list.push(entry)
      map.set(entry.page_index, list)
    }
    return [...map.entries()].sort(([a], [b]) => a - b)
  }, [manifest])

  return (
    <div className="flex flex-col gap-6">
      <div className="flex flex-wrap items-center gap-3">
        <Button onClick={() => openResultDir(resultDir)}>OPEN</Button>
        <span className="min-w-0 text-xs break-all text-muted-foreground">
          {resultDir}
        </span>
      </div>

      {summary && <p className="text-sm text-muted-foreground">{summary}</p>}

      {byPage.length === 0 ? (
        <p className="text-sm text-muted-foreground">
          No figures/tables/formulas/algorithm blocks were detected.
        </p>
      ) : (
        byPage.map(([pageIndex, entries]) => (
          <div key={pageIndex}>
            <h3 className="mb-2 text-sm font-medium text-muted-foreground">
              Page {pageIndex + 1}
            </h3>
            <ul className="divide-y border">
              {entries.map((entry) => (
                <li
                  key={entry.id}
                  className="flex flex-wrap items-center justify-between gap-2 px-3 py-2"
                >
                  <span
                    className={cn(
                      "text-xs",
                      KIND_LABEL_COLOR[entry.kind] ?? "text-foreground"
                    )}
                  >
                    {entry.kind} · {entry.id}
                  </span>
                  <VerificationBadge verification={entry.verification} />
                </li>
              ))}
            </ul>
          </div>
        ))
      )}
    </div>
  )
}
