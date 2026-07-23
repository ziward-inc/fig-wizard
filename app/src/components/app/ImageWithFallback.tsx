import { useState } from "react"
import { convertFileSrc } from "@tauri-apps/api/core"

import { basename } from "@/lib/format"
import { cn } from "@/lib/utils"

/**
 * Not every shipped format is guaranteed to render inline in every WebView
 * (WKWebView's AVIF support in particular has been inconsistent across
 * macOS/WebKit versions, and JPEG XL has none at all) - this falls back to
 * a generic file-icon + filename placeholder if the image actually fails
 * to load, checked empirically per-image rather than hardcoded per format.
 */
export function ImageWithFallback({
  path,
  alt,
  className,
}: {
  path: string
  alt: string
  className?: string
}) {
  const [failed, setFailed] = useState(false)

  if (failed) {
    return (
      <div
        className={cn(
          "flex h-[110px] w-full flex-col items-center justify-center gap-1 bg-muted p-2 text-center text-muted-foreground",
          className
        )}
      >
        <span className="text-xl">🗎</span>
        <span className="text-[0.7rem] break-all">{basename(path)}</span>
      </div>
    )
  }

  return (
    <img
      src={convertFileSrc(path)}
      alt={alt}
      className={cn("h-[110px] w-full bg-muted object-contain", className)}
      onError={() => setFailed(true)}
    />
  )
}
