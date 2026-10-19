import { useCallback, useEffect, useState } from 'react'
import type { FormEvent } from 'react'
import { useParams, useSearchParams } from 'react-router-dom'
import GridLayout, { WidthProvider } from 'react-grid-layout'
import { ApiError, api, isChart, setGuestBase } from '../api'
import type { GuestDashboard } from '../api'
import { Brand, GuestAuth } from '../auth'
import FilterBar from '../components/FilterBar'
import PeriodControl from '../components/PeriodControl'
import TileView from '../components/TileView'
import TextTile from '../components/TextTile'
import Tabs from '../components/Tabs'
import { onTab, pickTab } from '../tabs'
import { useDashboardInputs, initialFilterValues } from '../dashboardInputs'
import { restrictionsLabel } from '../guest'
import { usePhone } from '../hooks'
import { periodFromUrl } from '../period'
import { phoneLayout } from '../phoneLayout'
import { useTheme, useThemeOverride } from '../theme'

const Grid = WidthProvider(GridLayout)

/** A guest link (/g/<org>/<token>): its password, then the dashboard at the guest's role, within
 *  the link's limits, until it expires. No account involved. */
export default function GuestPage() {
  const { org = '', token = '' } = useParams()
  const base = `/api/guest/${encodeURIComponent(org)}/${encodeURIComponent(token)}`
  const [data, setData] = useState<GuestDashboard | null>(null)
  const [state, setState] = useState<'loading' | 'password' | 'gone' | 'ready'>('loading')
  const [error, setError] = useState<string | null>(null)
  const { adopt } = useTheme()

  // this page's tiles, filters and periods go through the link
  useEffect(() => { setGuestBase(base); return () => setGuestBase(null) }, [base])
  useEffect(() => {  // not for search engines (the server says so too)
    const meta = Object.assign(document.createElement('meta'), { name: 'robots', content: 'noindex' })
    document.head.appendChild(meta)
    return () => meta.remove()
  }, [])

  const load = useCallback(() => {
    api.guestDashboard(base).then(
      d => { if (d.theme) adopt(d.theme); setData(d); setState('ready') },
      e => {
        if (e instanceof ApiError && e.status === 401) setState('password')
        else { setError(e.message); setState('gone') }
      })
  }, [base, adopt])
  useEffect(load, [load])
  useThemeOverride(data?.theme?.id)

  if (state === 'loading') return <div className="page"><div className="empty">Loading…</div></div>
  if (state === 'gone') return <div className="public-gone"><h1>This link doesn't work</h1><p className="muted">{error}</p></div>
  if (state === 'password' || !data) return <Unlock base={base} onUnlocked={load} />
  return <GuestAuth permissions={data.permissions}><GuestDashboardView data={data} /></GuestAuth>
}

function Unlock({ base, onUnlocked }: { base: string; onUnlocked: () => void }) {
  const [password, setPassword] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const submit = async (e: FormEvent) => {
    e.preventDefault(); setBusy(true); setError(null)
    try { await api.guestUnlock(base, password); onUnlocked() } catch (x) { setError((x as Error).message) } finally { setBusy(false) }
  }
  return (
    <div className="login-wrap">
      <form className="login-card" onSubmit={submit}>
        <Brand big />
        <h1>A dashboard was shared with you</h1>
        <p className="muted">Enter the password you were given with this link.</p>
        <label>Password<input type="password" autoComplete="off" autoFocus value={password} onChange={e => setPassword(e.target.value)} required /></label>
        {error && <div className="error">{error}</div>}
        <button className="primary login-btn" disabled={busy || !password}>{busy ? 'Checking…' : 'Open the dashboard'}</button>
      </form>
    </div>
  )
}

function GuestDashboardView({ data }: { data: GuestDashboard }) {
  const dash = data.dashboard
  const phone = usePhone()
  const { filterValues, setFilterValues, period, setPeriod, filtersByModel, tilePeriod, tileRequest } = useDashboardInputs(dash)
  const [search, setSearch] = useSearchParams()
  useEffect(() => {
    setFilterValues(initialFilterValues(dash))
    setPeriod(dash.period ? periodFromUrl(dash.period, location.search) : null)
  }, [dash, setFilterValues, setPeriod])
  useEffect(() => { document.title = `${dash.title} · Magnum BI` }, [dash])

  const limits = restrictionsLabel(data.filters, data.models)
  const tabs = dash.tabs ?? []
  const tab = pickTab(tabs, search.get('tab'))
  const visible = onTab(dash.tiles, tabs, tab)
  const layout = phone ? phoneLayout(visible) : visible.map(t => ({ i: t.id, ...t.layout }))
  return (
    <div className="page public">
      <div className="public-head">
        <h1>{dash.title}</h1>
        {dash.description && <p className="muted">{dash.description}</p>}
        <p className="muted small">Shared by {data.shared_by} until {new Date(data.expires_at).toLocaleString()}
          {limits && <> · you see only {limits}</>}</p>
      </div>
      {period && (
        <div className="period-bar">
          <PeriodControl model={data.models.find(m => m.name === period.model)} period={period}
            onChange={p => setPeriod(p ? { ...p, model: period.model } : null)} />
        </div>
      )}
      <FilterBar filters={dash.filters} models={data.models} values={filterValues} dashboardId={data.id}
        onChange={(k, v) => setFilterValues(s => ({ ...s, [k]: v }))} />
      <Tabs tabs={tabs} current={tab} onPick={t => setSearch(p => { p.set('tab', t); return p }, { replace: true })} />
      <Grid className="grid" layout={layout} cols={12} rowHeight={64} margin={[12, 12]} isDraggable={false} isResizable={false} compactType="vertical">
        {visible.map(t => (
          <div key={t.id}>
            {!isChart(t) ? <TextTile title={t.title} text={t.text ?? ''} /> : <TileView tile={t} model={data.models.find(m => m.name === t.query.model)} extraFilters={filtersByModel[t.query.model]}
              period={period && period.model === t.query.model ? tilePeriod : null}
              source={{ dashboard: data.id, request: tileRequest }} />}
          </div>
        ))}
      </Grid>
      <p className="center muted small">Made with <a href="https://giobinda.github.io/magnumbi/" target="_blank" rel="noreferrer">Magnum BI</a></p>
    </div>
  )
}
