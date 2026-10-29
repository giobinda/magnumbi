import { ask } from '../dialogs'
import { useState } from 'react'
import { api } from '../api'
import type { TileRequest } from '../api'
import { saveFile } from '../download'

/** Every chart's data, with the filters and period on screen: a zip of CSVs, a folder per tab. */
export default function DownloadButton({ dashboard, request }: { dashboard: string; request: TileRequest }) {
  const [busy, setBusy] = useState(false)
  const download = async () => {
    setBusy(true)
    try { const { blob, name } = await api.download(dashboard, null, request); saveFile(blob, name) } catch (e) { void ask.alert(`Download failed: ${(e as Error).message}`) }
    setBusy(false)
  }
  return <button data-tour="download" title="Every chart's data as CSV files (a zip), with the filters and period on screen" disabled={busy} onClick={download}>{busy ? 'Preparing…' : 'Download'}</button>
}
