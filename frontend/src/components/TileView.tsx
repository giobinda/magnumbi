import { tileStats } from '../tileStats'
import { useMemo, useRef, useState } from 'react'
import * as echarts from 'echarts'
import type { ReactNode } from 'react'
import type { ChartTile, Column, Filter, ModelInfo, Period, TileRequest } from '../api'
import { drillPath, fieldLabel } from '../api'
import { applyDrill, nextDrill } from '../drill'
import type { DrillStep } from '../drill'
import { windowLabel } from '../period'
import { useQuery, useTileQuery } from '../hooks'
import { useCan } from '../auth'
import { api } from '../api'
import { blobOf, hasPicture, saveFile } from '../download'
import Chart from './Chart'
import type { DrillClick } from './Chart'

type Props = {
  tile: ChartTile
  model?: ModelInfo
  extraFilters?: Filter[]
  /** dashboard-wide period; a tile's own query.period wins, `options.period: false` opts out */
  period?: Period | null
  /** saved-dashboard mode: run the saved tile on the server with these viewer inputs */
  source?: { dashboard: string; request: TileRequest }
  editing?: boolean
  onEdit?: () => void
  onDelete?: () => void
  /** more buttons while editing (move to another tab) */
  actions?: ReactNode
}

export function tileTitle(tile: ChartTile, model?: ModelInfo, columns?: Column[]) {
  if (tile.title) return tile.title
  if (columns?.length) {  // the server's labels: they know ad-hoc aggregations too
    const m = columns.filter(c => c.kind === 'measure' && !c.compare).map(c => c.label).join(', ')
    const d = columns.filter(c => c.kind === 'dimension').map(c => c.label).join(', ')
    if (m || d) return [m, d].filter(Boolean).join(' by ')
  }
  const m = (tile.query.measures ?? []).map(x => fieldLabel(model, x)).join(', ')
  const d = (tile.query.dimensions ?? []).map(x => fieldLabel(model, x)).join(', ')
  return [m, d].filter(Boolean).join(' by ') || tile.id
}

export default function TileView({ tile, model, extraFilters, period, source, editing, onEdit, onDelete, actions }: Props) {
  const can = useCan()
  const [steps, setSteps] = useState<DrillStep[]>([])
  const [showSql, setShowSql] = useState(false)
  const path = drillPath(tile, model)

  const effectivePeriod = tile.query.period ?? (tile.options?.period === false ? null : period ?? null)
  const query = useMemo(() => {
    const q = applyDrill(tile.query, steps)
    return { ...q, filters: [...(q.filters ?? []), ...(extraFilters ?? [])], period: effectivePeriod }
  }, [tile.query, steps, extraFilters, effectivePeriod])
  // viewers & users run the saved tile (the server rebuilds it); builders editing run free-form
  const saved = Boolean(source && !editing)
  const free = useQuery(model && !saved ? query : null)
  const tileReq = useMemo(() => ({ ...source?.request, drill: steps.map(s => s.click) }), [source, steps])
  const stored = useTileQuery(saved ? source!.dashboard : null, tile.id, tileReq)
  const { data, error, status, loading } = saved ? stored : free
  const box = useRef<HTMLDivElement>(null)

  // the data as on screen (filters, period, drill), from the server; the chart as drawn
  const download = async (what: string) => {
    try {
      if (what === 'csv' && source) {
        const { blob, name } = await api.download(source.dashboard, tile.id, tileReq)
        saveFile(blob, name)
      } else if (what === 'png') {
        const el = box.current?.querySelector<HTMLElement>('[_echarts_instance_]')
        const chart = el && echarts.getInstanceByDom(el)
        if (!chart) return
        const background = getComputedStyle(el).getPropertyValue('--surface').trim() || '#fff'
        const blob = blobOf(chart.getDataURL({ type: 'png', pixelRatio: 2, backgroundColor: background }))
        saveFile(blob, `${tile.id}.png`)
      }
    } catch (e) { alert(`Download failed: ${(e as Error).message}`) }
  }

  const canDrill = !editing && can('drill') && (path.length > 0 ||
    (tile.query.dimensions ?? []).some(d => d.includes(':') && d.split(':')[0] !== effectivePeriod?.field))
  const onDrill = canDrill
    ? (c: DrillClick) => {
        const step = nextDrill(c.column, c.value, path, effectivePeriod?.field)
        if (step) setSteps(s => [...s, step])
      }
    : undefined

  return (
    <div className="tile" ref={box}>
      <div className="tile-head">
        <div className="tile-title" title={[tileTitle(tile, model, data?.columns), data && !editing ? tileStats(data) : ''].filter(Boolean).join('\n')}>
          {tileTitle(tile, model, data?.columns)}
          {steps.length > 0 && (
            <span className="crumbs">
              <button className="link" onClick={() => setSteps([])}>All</button>
              {steps.map((s, i) => (
                <span key={i}> › <button className="link" disabled={i === steps.length - 1} onClick={() => setSteps(steps.slice(0, i + 1))}>{s.label}</button></span>
              ))}
            </span>
          )}
        </div>
        <div className="tile-actions">
          {loading && <span className="spinner" title="Loading" />}
          {canDrill && steps.length === 0 && <span className="hint" title="Click a data point to drill down">⤓ drill</span>}
          {saved && data && can('download') && (
            <select className="tile-menu" data-tour="tile-menu" title="Download" value="" onChange={e => { const v = e.target.value; e.target.value = ''; download(v) }}>
              <option value="">⤓</option>
              <option value="csv">Data (CSV)</option>
              {hasPicture(tile.type) && <option value="png">Picture (PNG)</option>}
            </select>
          )}
          {can('view_sql') && <button className="icon" title="Show SQL" onClick={() => setShowSql(s => !s)}>{'</>'}</button>}
          {editing && actions}
          {editing && onEdit && <button className="icon" title="Edit tile" onClick={onEdit}>✎</button>}
          {editing && onDelete && <button className="icon" title="Remove tile" onClick={onDelete}>✕</button>}
        </div>
      </div>
      <div className="tile-body">
        {!model ? <div className="error">Unknown model “{tile.query.model}”</div>
          : error ? <div className={status === 413 ? 'warn' : 'error'}>{error}</div>
          : showSql && data?.sql ? <pre className="sql">{data.sql}</pre>
          : data ? <Chart result={data} type={tile.type} options={tile.options} onDrill={onDrill} />
          : <div className="empty">Loading…</div>}
      </div>
      {/* rows and timings matter while building; on the dashboard they're in the title's tooltip */}
      {data && editing && !['kpi', 'trend', 'gauge'].includes(tile.type) && (
        <div className="tile-foot">
          {data.rows.length} rows · {data.stats.duration_ms ?? '–'} ms
          {data.stats.cache_hit && <span title={data.stats.cached_at ? `Cached ${new Date(data.stats.cached_at).toLocaleString()}` : undefined}> · cached</span>}
          {data.period?.previous && <span title={`Current: ${windowLabel(data.period.current)}`}> · vs {windowLabel(data.period.previous)}</span>}
        </div>
      )}
    </div>
  )
}
