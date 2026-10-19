/** Downloads: data as CSV (a tile) or a zip of CSVs (a dashboard), from the server; a chart as
 *  a PNG, from the browser. */

/** The file name in a Content-Disposition header, else `fallback`. */
export function fileNameOf(disposition: string | null, fallback: string): string {
  return disposition?.match(/filename="([^"]+)"/)?.[1] ?? fallback
}

/** Charts drawn with ECharts can be saved as a picture; numbers and tables are data. */
export function hasPicture(type: string): boolean {
  return !['kpi', 'table', 'pivot', 'trend', 'text'].includes(type)
}

/** A base64 data URL (a chart's picture) as a file. Not fetch(): the CSP's connect-src is 'self'. */
export function blobOf(dataUrl: string): Blob {
  const [head, data] = dataUrl.split(',', 2)
  const bytes = Uint8Array.from(atob(data), c => c.charCodeAt(0))
  return new Blob([bytes], { type: head.match(/^data:([^;]+)/)?.[1] ?? 'application/octet-stream' })
}

/** Hand the browser a file to save. */
export function saveFile(blob: Blob, name: string) {
  const url = URL.createObjectURL(blob)
  const a = Object.assign(document.createElement('a'), { href: url, download: name })
  document.body.appendChild(a)
  a.click()
  a.remove()
  setTimeout(() => URL.revokeObjectURL(url), 1000)
}
