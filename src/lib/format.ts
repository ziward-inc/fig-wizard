export const KIND_ORDER = [
  "figure",
  "table",
  "formula",
  "algorithm",
  "aside",
  "seal",
] as const

export function pdfStem(path: string): string {
  const base = path.split("/").pop() || path
  return base.replace(/\.pdf$/i, "")
}

export function dirName(path: string): string {
  const idx = path.lastIndexOf("/")
  return idx >= 0 ? path.slice(0, idx) : "."
}

export function formatBytes(n: number): string {
  if (!n || n <= 0) return "0 MB"
  return `${(n / (1024 * 1024)).toFixed(1)} MB`
}
