import { useState } from 'react'
import { useSearchParams } from 'react-router-dom'
import { api } from '../api'

/** The link at the bottom of a scheduled report. A button, not a visit: mail scanners open links. */
export default function UnsubscribePage() {
  const [params] = useSearchParams()
  const org = params.get('o') ?? ''  // empty: the platform workspace
  const token = params.get('t') ?? ''
  const [done, setDone] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const stop = async () => {
    try { const r = await api.unsubscribe(org, token); setDone(`${r.email} won't receive “${r.report}” any more.`) } catch (e) { setError((e as Error).message) }
  }
  return (
    <div className="public-gone">
      <h1>Stop this report</h1>
      {done ? <p>{done}</p> : <>
        <p className="muted">Stop receiving this scheduled report by email. Its other recipients still get it.</p>
        <button className="primary" onClick={stop}>Stop sending it to me</button>
      </>}
      {error && <div className="error">{error}</div>}
    </div>
  )
}
