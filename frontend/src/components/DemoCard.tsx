import { useEffect, useState } from 'react'
import { api, setOrg } from '../api'
import type { DemoStatus, PlatformResponse } from '../api'

const MONTHS: [number, string][] = [[1, '1 month'], [13, '13 months (a month and a year earlier)'], [24, '2 years'], [60, '5 years']]

/** System owners: set up the NYC demo (taxi trips x weather) for an organisation, in the background.
 *  With no organisations to choose from (lite, the platform's "Get started"): the platform workspace.
 *  `months`: what it offers first (13: a month and a year earlier, for the comparisons). */
export default function DemoCard({ orgs = [], databases = {}, months: first = 13, onDone }: {
  orgs?: { id: string; name: string }[]; databases?: PlatformResponse['databases']; months?: number
  /** called once it's set up (Get started then ticks its steps) */ onDone?: () => void
}) {
  const [org, setOrgChoice] = useState('')
  const [months, setMonths] = useState(first)
  const [status, setStatus] = useState<DemoStatus | null>(null)
  const [err, setErr] = useState<string | null>(null)
  useEffect(() => { api.demoStatus().then(setStatus, e => setErr(e.message)) }, [])
  useEffect(() => {
    if (!status?.running) return
    const t = setTimeout(() => { api.demoStatus().then(setStatus, e => setErr(e.message)) }, 2000)
    return () => clearTimeout(t)
  }, [status])

  const start = async () => {
    setErr(null)
    try { setStatus(await api.startDemo(org, months)) } catch (e) { setErr((e as Error).message) }
  }
  const open = (target: string) => { setOrg(target || null); location.assign('/dashboards/nyc_taxi_weather') }
  const finished = status && !status.running && status.step === 'Done'
  useEffect(() => { if (finished) onDone?.() }, [finished, onDone])
  return (
    <div className="card person demo-card">
      <p className="small muted">New York yellow taxi trips (NYC TLC open data) with the hourly weather in each borough (Open-Meteo),
        two DuckDB connections, and the dashboards <b>NYC taxi overview</b> and <b>NYC taxi × weather</b>, joined with the blend
        taxi_weather. About 60 MB of downloads a month; months already there are skipped.</p>
      <div className="row wrap">
        {orgs.length > 0 && <select value={org} onChange={e => setOrgChoice(e.target.value)} disabled={status?.running}>
          <option value="">Platform workspace</option>
          {orgs.map(o => <option key={o.id} value={o.id} disabled={!databases[o.id]}>{o.name}{databases[o.id] ? '' : ' (no database yet)'}</option>)}
        </select>}
        <select value={months} onChange={e => setMonths(Number(e.target.value))} disabled={status?.running}>
          {MONTHS.map(([n, l]) => <option key={n} value={n}>{l} · ~{Math.round(n * 0.06 * 10) / 10} GB</option>)}
        </select>
        <button className="primary" onClick={start} disabled={status?.running}>{status?.running ? 'Setting up…' : 'Set up the demo'}</button>
      </div>
      {status && (status.running || status.log.length > 0) && (
        <div className="demo-progress">
          <div className="small"><b>{status.step}</b>{status.total > 0 && ` · ${status.done} of ${status.total} months`}
            {status.org ? ` · ${orgs.find(o => o.id === status.org)?.name ?? status.org}` : orgs.length > 0 ? ' · platform workspace' : ''}</div>
          {status.total > 0 && <progress max={status.total} value={status.done} />}
          <pre className="small mono">{status.log.slice(-8).join('\n')}</pre>
          {status.error && <div className="error">{status.error}</div>}
          {finished && <button className="link" onClick={() => open(status.org)}>Open NYC taxi × weather</button>}
        </div>
      )}
      {err && <div className="error">{err}</div>}
    </div>
  )
}
