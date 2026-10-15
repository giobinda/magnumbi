import { useEffect, useMemo, useState } from 'react'
import { api } from '../api'
import type { CacheStats, Column, QueryResult, UsageSummary } from '../api'
import { useAuth } from '../auth'
import Chart from '../components/Chart'

const measure = (name: string, label: string, format = 'number'): Column =>
  ({ name, field: name, label, kind: 'measure', type: 'number', format })

/** Usage of the organisation (Org admins) or the whole platform (system owners in the platform
 *  workspace), from the usage metrics in the primary ScramDB; plus the organisation's query cache. */
export default function UsagePage() {
  const { auth } = useAuth()
  const [days, setDays] = useState(30)
  const [data, setData] = useState<UsageSummary | null>(null)
  const [error, setError] = useState<string | null>(null)
  useEffect(() => { api.usage(days).then(setData, e => setError(e.message)) }, [days])

  const trend: QueryResult | null = useMemo(() => data && {
    columns: [
      { name: 'day', field: 'day:day', label: 'Day', kind: 'dimension', type: 'date', grain: 'day' },
      measure('queries', 'Queries'), measure('dashboard_views', 'Dashboard views'), measure('people', 'Active people'),
    ],
    rows: data.by_day.map(d => [d.day, d.queries, d.dashboard_views, d.people]),
    stats: {}, sql: null,
  }, [data])

  if (error) return <div className="page"><div className="error">{error}</div></div>
  if (!data || !trend) return <div className="page"><div className="empty">Loading…</div></div>
  const t = data.totals
  const pct = (v: number | null) => (v === null ? '–' : `${Math.round(v * 100)}%`)
  const platform = data.scope === 'platform'
  return (
    <div className="page">
      <div className="dash-head">
        <h1>Usage <span className="muted">· {platform ? 'whole platform' : auth?.org?.name ?? data.scope}</span></h1>
        <div className="dash-actions">
          <select value={days} onChange={e => setDays(Number(e.target.value))}>
            {[7, 30, 90, 365].map(d => <option key={d} value={d}>Last {d} days</option>)}
          </select>
        </div>
      </div>

      <div className="cards usage-kpis">
        {([['Active people', t.people], ['Queries', t.queries], ['From cache', pct(t.cache_hit_rate)],
           ['Avg query time', t.avg_query_ms === null ? '–' : `${Math.round(t.avg_query_ms)} ms`],
           ['Dashboard views', t.dashboard_views], ['Failed queries', t.failed_queries], ['Sign-ins', t.sign_ins]] as const).map(([label, v]) => (
          <div key={label} className="card kpi-item"><div className="kpi-value">{v}</div><div className="kpi-label">{label}</div></div>
        ))}
      </div>

      <div className="card usage-chart"><Chart result={trend} type="line" /></div>

      {platform && data.orgs.length > 0 && <>
        <h2 className="section">Organisations</h2>
        <table className="data">
          <thead><tr><th>Organisation</th><th>Active people</th><th>Queries</th><th>Dashboard views</th></tr></thead>
          <tbody>{data.orgs.map(o => <tr key={o.org}><td>{o.org || 'Platform workspace'}</td><td>{o.people}</td><td>{o.queries}</td><td>{o.dashboard_views}</td></tr>)}</tbody>
        </table>
      </>}

      <div className="usage-grid">
        <div>
          <h2 className="section">Most viewed dashboards</h2>
          {data.dashboards.length === 0 ? <p className="muted small">No dashboard views yet.</p> : (
            <table className="data">
              <thead><tr><th>Dashboard</th>{platform && <th>Organisation</th>}<th>Views</th><th>People</th></tr></thead>
              <tbody>{data.dashboards.map(d => (
                <tr key={d.org + d.dashboard_id}><td>{d.dashboard_id}</td>{platform && <td>{d.org || '–'}</td>}<td>{d.views}</td><td>{d.people}</td></tr>
              ))}</tbody>
            </table>
          )}
        </div>
        <div>
          <h2 className="section">Models</h2>
          {data.models.length === 0 ? <p className="muted small">No queries yet.</p> : (
            <table className="data">
              <thead><tr><th>Model</th><th>Queries</th><th>From cache</th><th>Failed</th><th>Avg time</th></tr></thead>
              <tbody>{data.models.map(m => (
                <tr key={m.model ?? ''}><td>{m.model}</td><td>{m.queries}</td><td>{pct(m.queries ? m.cache_hits / m.queries : null)}</td>
                  <td>{m.failed}</td><td>{m.avg_ms === null ? '–' : `${Math.round(m.avg_ms)} ms`}</td></tr>
              ))}</tbody>
            </table>
          )}
        </div>
      </div>

      <h2 className="section">People</h2>
      <table className="data">
        <thead><tr><th>Person</th>{platform && <th>Organisation</th>}<th>Role</th><th>Queries</th><th>Dashboard views</th><th>Last seen</th></tr></thead>
        <tbody>{data.people.map(p => (
          <tr key={p.email + p.org}><td>{p.email}</td>{platform && <td>{p.org || '–'}</td>}<td>{p.role ?? '–'}</td><td>{p.queries}</td>
            <td>{p.dashboard_views}</td><td className="small">{new Date(p.last_seen).toLocaleString()}</td></tr>
        ))}</tbody>
      </table>

      {!platform && <QueryCache />}
    </div>
  )
}

/** The organisation's query cache, in its own ScramDB: what may be cached, what is, and a reset. */
function QueryCache() {
  const [stats, setStats] = useState<CacheStats | null>(null)
  const [msg, setMsg] = useState<string | null>(null)
  const load = () => api.cacheStats().then(setStats, e => setMsg(e.message))
  useEffect(() => { load() }, [])
  const clear = async (model?: string) => {
    try { const r = await api.clearCache(model); setMsg(`${r.cleared} cached result(s) removed.`); load() }
    catch (e) { setMsg((e as Error).message) }
  }
  const dur = (s: number) => (s === 0 ? 'off (always live)' : s % 3600 === 0 ? `${s / 3600} h` : s % 60 === 0 ? `${s / 60} min` : `${s} s`)
  return (
    <>
      <h2 className="section">Query cache</h2>
      <p className="muted small">Models with <code>cache:</code> reuse results for that long, stored in this organisation's own
        ScramDB. Clear a model's cache after loading new data.</p>
      {stats && (
        <table className="data">
          <thead><tr><th>Model</th><th>Reuse results for</th><th>Cached results</th><th>Rows</th><th>Hits</th><th /></tr></thead>
          <tbody>{Object.entries(stats.ttl_seconds).map(([m, ttl]) => {
            const c = stats.cached[m]
            return (
              <tr key={m}><td>{m}</td><td>{dur(ttl)}</td><td>{c?.results ?? 0}</td><td>{c?.rows ?? 0}</td><td>{c?.hits ?? 0}</td>
                <td>{c && <button className="link" onClick={() => clear(m)}>Clear</button>}</td></tr>
            )
          })}</tbody>
        </table>
      )}
      {msg && <p className="muted small">{msg}</p>}
    </>
  )
}
