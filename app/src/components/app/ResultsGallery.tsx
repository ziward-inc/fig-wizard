import { useMemo, useState } from "react"

import { ImageWithFallback } from "@/components/app/ImageWithFallback"
import { ObjectDialog } from "@/components/app/ObjectDialog"
import { VerificationBadge } from "@/components/app/VerificationBadge"
import { cn } from "@/lib/utils"
import type { Manifest, ManifestEntry } from "@/lib/tauri-types"

const KIND_LABEL_COLOR: Record<string, string> = {
  figure: "text-blue-700 dark:text-blue-400",
  table: "text-green-700 dark:text-green-400",
  formula: "text-amber-700 dark:text-amber-400",
  algorithm: "text-purple-700 dark:text-purple-400",
  aside: "text-slate-500 dark:text-slate-400",
}

export function ResultsGallery({ manifest }: { manifest: Manifest }) {
  const [selected, setSelected] = useState<ManifestEntry | null>(null)

  const byPage = useMemo(() => {
    const map = new Map<number, ManifestEntry[]>()
    for (const entry of manifest.objects) {
      const list = map.get(entry.page_index) ?? []
      list.push(entry)
      map.set(entry.page_index, list)
    }
    return [...map.entries()].sort(([a], [b]) => a - b)
  }, [manifest])

  if (byPage.length === 0) {
    return (
      <p className="text-sm text-muted-foreground">
        No figures/tables/formulas/algorithm blocks were detected.
      </p>
    )
  }

  return (
    <div className="flex flex-col gap-6">
      {byPage.map(([pageIndex, entries]) => (
        <div key={pageIndex}>
          <h3 className="mb-2 text-sm font-medium text-muted-foreground">
            Page {pageIndex + 1}
          </h3>
          <div className="grid grid-cols-[repeat(auto-fill,minmax(160px,1fr))] gap-3">
            {entries.map((entry) => (
              <button
                key={entry.id}
                type="button"
                className="relative flex cursor-pointer flex-col overflow-hidden border bg-card text-left"
                onClick={() => setSelected(entry)}
              >
                <ImageWithFallback
                  path={entry.files.with_caption}
                  alt={`${entry.kind} on page ${entry.page_index + 1}`}
                />
                <VerificationBadge
                  verification={entry.verification}
                  className="absolute top-1 right-1 shadow"
                />
                <div
                  className={cn(
                    "truncate border-t px-2 py-1 text-xs whitespace-nowrap",
                    KIND_LABEL_COLOR[entry.kind] ?? "text-foreground"
                  )}
                >
                  {entry.kind} · {entry.id}
                </div>
              </button>
            ))}
          </div>
        </div>
      ))}

      <ObjectDialog entry={selected} onOpenChange={(open) => !open && setSelected(null)} />
    </div>
  )
}
