import { useCallback, useEffect, useMemo, useState } from 'react'
import { Link, useNavigate, useParams } from 'react-router-dom'
import GridLayout, { WidthProvider } from 'react-grid-layout'
import type { Layout } from 'react-grid-layout'
import CodeMirror from '@uiw/react-codemirror'
import { yaml as yamlLang } from '@codemirror/lang-yaml'
import { ApiError, api, isTemporal } from '../api'
import type { Dashboard, DashboardPeriod, Filter, Tile, Value } from '../api'
import Explorer from '../components/Explorer'
import FilterBar, { filterKey } from '../components/FilterBar'
import PeriodControl from '../components/PeriodControl'
import ShareDialog from '../components/ShareDialog'
import { useTheme, useThemeOverride } from '../theme'
import { useAuth, useCan } from '../auth'
import type { OrgRef } from '../api'
import { periodFromUrl } from '../period'
import TileView from '../components/TileView'
import { useModels, usePhone } from '../hooks'
import { phoneLayout } from '../phoneLayout'

const Grid = WidthProvider(GridLayout)

function initialFilterValues(d: Dashboard): Record<string, Value[]> {
  // URL params override defaults: ?status=Complete,Shipped&created_at=30
  const params = new URLSearchParams(location.search)
  const out: Record<string, Value[]> = {}
  for (const f of d.filters) {
    const p = params.get(f.field)
    out[filterKey(f)] = p !== null ? (p === '' ? [] : p.split(',')) : (f.default ?? [])
  }
  return out
}

/** A save refused because the dashboard changed meanwhile: what to tell the person. */
const conflict = (e: unknown) => (e instanceof ApiError && e.status === 409
  ? 'Someone else changed this dashboard since you opened it. Reload the page to see their version (your unsaved changes will be lost), or copy your YAML first.'
  : null)

export default function DashboardPage({ embed = false }: { embed?: boolean }) {
  const params = useParams()
  const id = params.id ?? ''
  const navigate = useNavigate()
  const { models, error: modelsError } = useModels()
  const phone = usePhone()
  const [dash, setDash] = useState<Dashboard | null>(null)
  const [yamlText, setYamlText] = useState('')
  const [yamlDraft, setYamlDraft] = useState<string | null>(null)
  const [warnings, setWarnings] = useState<string[]>([])
  const [error, setError] = useState<string | null>(null)
  const [editing, setEditing] = useState(new URLSearchParams(location.search).has('edit') && !embed)
  const [dirty, setDirty] = useState(false)
  // the version loaded: saves are refused if someone (or an agent) changed it since
  const [version, setVersion] = useState<string | null>(null)
  const [view, setView] = useState<'dashboard' | 'yaml'>('dashboard')
  const [editTile, setEditTile] = useState<Tile | 'new' | null>(null)
  const [showEmbed, setShowEmbed] = useState(false)
  const [showShare, setShowShare] = useState(false)
  const [filterValues, setFilterValues] = useState<Record<string, Value[]>>({})
  const [period, setPeriod] = useState<DashboardPeriod | null>(null)  // viewer's current choice
  const [saving, setSaving] = useState(false)
  const can = useCan()
  const auth = useAuth().auth
  const allRestrictions = auth?.restrictions ?? []
  const [showPublish, setShowPublish] = useState(false)
  const isTemplate = can('manage_orgs') && !auth?.org  // platform workspace: dashboards are templates
  const { themes } = useTheme()
  // an embed's ?theme= wins (the host page decides), then the dashboard's pinned theme
  useThemeOverride(new URLSearchParams(location.search).get('theme') || dash?.theme || null)

  const load = useCallback(() => {
    api.dashboard(id).then(r => {
      setDash(r.dashboard); setYamlText(r.yaml ?? ''); setWarnings(r.warnings); setDirty(false); setYamlDraft(null)
      setVersion(r.version ?? null)
      setFilterValues(initialFilterValues(r.dashboard))
      setPeriod(r.dashboard.period ? periodFromUrl(r.dashboard.period, location.search) : null)
    }, e => setError(e.message))
  }, [id])
  useEffect(load, [load])

  // embedded: tell the host page how tall we are so it can size the iframe
  useEffect(() => {
    if (!embed) return
    const ro = new ResizeObserver(() => window.parent.postMessage({ type: 'magnum:resize', id, height: document.documentElement.scrollHeight }, '*'))
    ro.observe(document.body)
    return () => ro.disconnect()
  }, [embed, id])

  const filtersByModel = useMemo(() => {
    const out: Record<string, Filter[]> = {}
    for (const f of dash?.filters ?? []) {
      const values = filterValues[filterKey(f)] ?? []
      const op = f.op ?? 'equals'
      if (!values.length && op !== 'is_null' && op !== 'not_null') continue
      ;(out[f.model] ??= []).push({ field: f.field, op, values })
    }
    return out
  }, [dash, filterValues])

  // the query-level period has no `model`; it only applies to tiles on that model
  const tilePeriod = useMemo(() => {
    if (!period) return null
    const { model: _model, ...p } = period
    return p
  }, [period])

  // what a saved tile may be asked for: filter values and the period window (drill is per tile)
  const tileRequest = useMemo(() => ({
    filters: filterValues,
    period: period ? { current: period.current, compare: period.compare ?? null } : null,
  }), [filterValues, period])

  const update = (fn: (d: Dashboard) => Dashboard) => { setDash(d => (d ? fn(d) : d)); setDirty(true) }

  // viewers change the period freely; in edit mode the change also becomes the saved default
  const changePeriod = (p: DashboardPeriod | null) => {
    setPeriod(p)
    if (editing) update(d => ({ ...d, period: p }))
  }
  const addPeriod = () => {
    const modelName = dash?.tiles[0]?.query.model ?? models[0]?.name
    const m = models.find(x => x.name === modelName)
    const field = m?.dimensions.find(d => isTemporal(d.type))?.name
    if (!m || !field) { alert('No date/time dimension available on this dashboard\'s model.'); return }
    changePeriod({ model: m.name, field, current: { last_n_days: 30 }, compare: 'previous_period' })
  }

  const save = async () => {
    if (!dash) return
    setSaving(true)
    try {
      const r = await api.saveDashboard(id, dash, version)
      setYamlText(r.yaml ?? ''); setWarnings(r.warnings); setDirty(false); setEditing(false); setVersion(r.version ?? null)
    } catch (e) { alert(conflict(e) ?? `Save failed: ${(e as Error).message}`) }
    setSaving(false)
  }

  const applyYaml = async () => {
    if (yamlDraft === null) return
    try {
      const r = await api.saveDashboardYaml(id, yamlDraft, version)
      setDash(r.dashboard); setYamlText(r.yaml ?? ''); setWarnings(r.warnings); setYamlDraft(null); setDirty(false)
      setVersion(r.version ?? null)
      setFilterValues(initialFilterValues(r.dashboard))
      setPeriod(r.dashboard.period ?? null)
    } catch (e) { alert(conflict(e) ?? `Invalid dashboard: ${(e as Error).message}`) }
  }

  const onLayoutChange = (layout: Layout[]) => {
    if (!editing || !dash || phone) return // the phone layout is for display, never saved
    const changed = layout.some(l => {
      const t = dash.tiles.find(x => x.id === l.i)
      return t && (t.layout.x !== l.x || t.layout.y !== l.y || t.layout.w !== l.w || t.layout.h !== l.h)
    })
    if (changed) update(d => ({ ...d, tiles: d.tiles.map(t => { const l = layout.find(x => x.i === t.id)!; return { ...t, layout: { x: l.x, y: l.y, w: l.w, h: l.h } } }) }))
  }

  const upsertTile = (tile: Tile) => {
    update(d => {
      if (d.tiles.some(t => t.id === tile.id) && editTile !== 'new') return { ...d, tiles: d.tiles.map(t => (t.id === tile.id ? tile : t)) }
      const bottom = Math.max(0, ...d.tiles.map(t => t.layout.y + t.layout.h))
      const newId = uniqueId(tile.title || tile.query.measures?.[0] || 'tile', d.tiles.map(t => t.id))
      return { ...d, tiles: [...d.tiles, { ...tile, id: newId, layout: { x: 0, y: bottom, w: tile.type === 'kpi' ? 3 : 6, h: tile.type === 'kpi' ? 2 : 5 } }] }
    })
    setEditTile(null)
  }

  if (error) return <div className="page"><div className="error">{error}</div><Link to="/">← Dashboards</Link></div>
  if (!dash) return <div className="page"><div className="empty">Loading…</div></div>

  const layout: Layout[] = phone ? phoneLayout(dash.tiles) : dash.tiles.map(t => ({ i: t.id, ...t.layout, minW: 2, minH: 2 }))
  const showFilters = !embed || new URLSearchParams(location.search).get('filters') !== '0'

  return (
    <div className={embed ? 'page embed' : 'page'}>
      {!embed && (
        <div className="dash-head">
          <div>
            {editing
              ? <input className="title-edit" value={dash.title} onChange={e => update(d => ({ ...d, title: e.target.value }))} />
              : <h1>{dash.title}</h1>}
            {dash.description && !editing && <p className="muted">{dash.description}</p>}
            {(() => {  // only the restrictions on models this dashboard uses
              const used = new Set(dash.tiles.map(t => t.query.model))
              const shown = allRestrictions.filter(r => used.has(r.model) || models.some(m => used.has(m.name) && m.extends === r.model))
              return shown.length > 0 && <p className="muted small">🔒 Showing only {shown.map(r => `${r.label}: ${r.values.join(', ')}`).join(' · ')}</p>
            })()}
          </div>
          <div className="dash-actions">
            {can('edit') && <div className="seg">
              <button className={view === 'dashboard' ? 'active' : ''} onClick={() => setView('dashboard')}>Dashboard</button>
              <button className={view === 'yaml' ? 'active' : ''} onClick={() => setView('yaml')}>YAML</button>
            </div>}
            {view === 'dashboard' && (editing ? <>
              <select className="theme-select" title="Theme for everyone who opens this dashboard" value={dash.theme ?? ''}
                onChange={e => update(d => ({ ...d, theme: e.target.value || null }))}>
                <option value="">Theme: viewer's choice</option>
                {themes.map(t => <option key={t.id} value={t.id}>Theme: {t.label}</option>)}
              </select>
              <button onClick={() => setEditTile('new')}>+ Add tile</button>
              <button onClick={() => { if (!dirty || confirm('Discard changes?')) { load(); setEditing(false) } }}>Cancel</button>
              <button className="primary" disabled={saving} onClick={save}>{saving ? 'Saving…' : 'Save'}</button>
            </> : <>
              {isTemplate && <button title="Copy this template into organisations" onClick={() => setShowPublish(true)}>Publish…</button>}
              {can('share') && <button onClick={() => setShowEmbed(true)}>Embed</button>}
              {can('manage_members') && auth?.org && <button onClick={() => setShowShare(true)}>Share publicly</button>}
              {can('edit') && <button onClick={() => setEditing(true)}>Edit</button>}
              {can('edit') && <button className="danger" title="Delete dashboard" onClick={async () => {
                if (confirm(`Delete dashboard "${dash.title}"? Its history is kept.`)) { await api.deleteDashboard(id); navigate('/') }
              }}>🗑</button>}
            </>)}
          </div>
        </div>
      )}

      {modelsError && <div className="error">Could not load models: {modelsError}</div>}
      {warnings.length > 0 && !embed && <div className="warn">{warnings.map(w => <div key={w}>⚠ {w}</div>)}</div>}

      {view === 'yaml' && !embed && can('edit') ? (
        <div className="yaml-view">
          <p className="muted small">The dashboard as YAML. Edit and apply here, or download it as <code>{id}.yaml</code> to keep in git.</p>
          <CodeMirror value={yamlDraft ?? yamlText} height="calc(100vh - 260px)" extensions={[yamlLang()]} onChange={setYamlDraft} />
          <div className="row">
            <button disabled={yamlDraft === null} onClick={() => setYamlDraft(null)}>Revert</button>
            <a className="btn" href={`/api/dashboards/${id}/yaml`} download={`${id}.yaml`}>Download</a>
            <button className="primary" disabled={yamlDraft === null || dirty} title={dirty ? 'Save or cancel visual edits first' : ''} onClick={applyYaml}>Apply YAML</button>
          </div>
        </div>
      ) : (
        <>
          {showFilters && period && (
            <div className="period-bar">
              <PeriodControl model={models.find(m => m.name === period.model)} period={period}
                onChange={p => changePeriod(p ? { ...p, model: period.model } : null)} />
              {editing && <button className="link" onClick={() => changePeriod(null)}>Remove period</button>}
            </div>
          )}
          {editing && !period && <div className="period-bar"><button className="link" onClick={addPeriod}>+ Add a dashboard period (with comparison)</button></div>}
          {showFilters && <FilterBar filters={dash.filters} models={models} values={filterValues} dashboardId={editing ? undefined : id}
            onChange={(k, v) => setFilterValues(s => ({ ...s, [k]: v }))} />}
          {dash.tiles.length === 0 && <div className="empty big">This dashboard has no tiles yet. {!embed && can('edit') && <button className="link" onClick={() => { setEditing(true); setEditTile('new') }}>Add one</button>}</div>}
          <Grid className={editing ? 'grid editing' : 'grid'} layout={layout} cols={12} rowHeight={64} margin={[12, 12]}
            isDraggable={editing && !phone} isResizable={editing && !phone} draggableHandle=".tile-head" draggableCancel="button,select,input"
            onLayoutChange={onLayoutChange} compactType="vertical">
            {dash.tiles.map(t => (
              <div key={t.id}>
                <TileView tile={t} model={models.find(m => m.name === t.query.model)} extraFilters={filtersByModel[t.query.model]}
                  period={period && period.model === t.query.model ? tilePeriod : null}
                  source={{ dashboard: id, request: tileRequest }}
                  editing={editing} onEdit={() => setEditTile(t)}
                  onDelete={() => update(d => ({ ...d, tiles: d.tiles.filter(x => x.id !== t.id) }))} />
              </div>
            ))}
          </Grid>
        </>
      )}

      {editTile && (
        <div className="modal-bg" onMouseDown={e => { if (e.target === e.currentTarget) setEditTile(null) }}>
          <div className="modal wide">
            <h2>{editTile === 'new' ? 'Add tile' : 'Edit tile'}</h2>
            <Explorer models={models} initial={editTile === 'new' ? undefined : editTile}
              submitLabel={editTile === 'new' ? 'Add to dashboard' : 'Apply'} onSubmit={upsertTile} onCancel={() => setEditTile(null)} />
          </div>
        </div>
      )}

      {showEmbed && <EmbedDialog id={id} onClose={() => setShowEmbed(false)} />}
      {showShare && auth?.org && <ShareDialog id={id} org={auth.org.name} tiles={dash.tiles.length} onClose={() => setShowShare(false)} />}
      {showPublish && <PublishDialog id={id} orgs={auth?.orgs ?? []} onClose={() => setShowPublish(false)} />}
    </div>
  )
}

/** System owners: copy a platform template into organisations (each copy is then theirs). */
function PublishDialog({ id, orgs, onClose }: { id: string; orgs: OrgRef[]; onClose: () => void }) {
  const [chosen, setChosen] = useState<string[]>([])
  const [as, setAs] = useState(id)
  const [overwrite, setOverwrite] = useState(false)
  const [results, setResults] = useState<Record<string, string> | null>(null)
  const [error, setError] = useState<string | null>(null)
  const publish = async () => {
    setError(null)
    try { setResults((await api.publishTemplate(id, chosen, as, overwrite)).results) } catch (e) { setError((e as Error).message) }
  }
  return (
    <div className="modal-bg" onMouseDown={e => { if (e.target === e.currentTarget) onClose() }}>
      <div className="modal publish">
        <h2>Publish to organisations</h2>
        <p className="muted small">Each organisation gets its own copy, which its builders can then change. Organisations
          without the models this dashboard uses are skipped.</p>
        {orgs.length === 0 && <p className="warn">No organisations yet.</p>}
        {orgs.map(o => (
          <label key={o.id} className="check">
            <input type="checkbox" checked={chosen.includes(o.id)}
              onChange={e => setChosen(e.target.checked ? [...chosen, o.id] : chosen.filter(x => x !== o.id))} /> {o.name}
            {results?.[o.id] && <span className={results[o.id] === 'published' ? 'ok small' : 'muted small'}> {results[o.id]}</span>}
          </label>
        ))}
        <label className="small">Dashboard id in the organisations <input value={as} onChange={e => setAs(e.target.value)} /></label>
        <label className="check small"><input type="checkbox" checked={overwrite} onChange={e => setOverwrite(e.target.checked)} />
          Replace an existing dashboard with this id</label>
        {error && <div className="error">{error}</div>}
        <div className="row">
          <button onClick={onClose}>Close</button>
          <button className="primary" disabled={chosen.length === 0} onClick={publish}>Publish</button>
        </div>
      </div>
    </div>
  )
}

function uniqueId(base: string, taken: string[]) {
  const slug = base.toLowerCase().replace(/[^a-z0-9]+/g, '_').replace(/^_|_$/g, '').slice(0, 40) || 'tile'
  let id = slug, n = 2
  while (taken.includes(id)) id = `${slug}_${n++}`
  return id
}

function EmbedDialog({ id, onClose }: { id: string; onClose: () => void }) {
  const origin = location.origin
  const iframe = `<iframe src="${origin}/embed/${id}" style="width:100%;height:800px;border:0"></iframe>`
  const wc = `<script src="${origin}/embed.js"></script>\n<magnum-dashboard dashboard="${id}" filters="category=Footwear"></magnum-dashboard>`
  return (
    <div className="modal-bg" onMouseDown={e => { if (e.target === e.currentTarget) onClose() }}>
      <div className="modal">
        <h2>Embed this dashboard</h2>
        <p className="muted small">Web component (auto-resizes; <code>filters</code> sets dashboard filters by field name, <code>hide-filters</code> hides the bar):</p>
        <pre className="code">{wc}</pre>
        <p className="muted small">Or a plain iframe (filters as URL params, e.g. <code>?category=Footwear&amp;period=30&amp;compare=previous_year&amp;filters=0</code>):</p>
        <pre className="code">{iframe}</pre>
        <p className="muted small">Allowed host sites are controlled by <code>MAGNUM_FRAME_ANCESTORS</code> on the server.</p>
        <div className="row"><a className="btn" href={`/embed/${id}`} target="_blank" rel="noreferrer">Open embed view</a><button className="primary" onClick={onClose}>Close</button></div>
      </div>
    </div>
  )
}
