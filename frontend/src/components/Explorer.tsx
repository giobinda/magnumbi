import { useEffect, useMemo, useState } from 'react'
import type { DragEvent, ReactNode } from 'react'
import { AGG_LABELS, fieldGrain, fieldLabel, fieldName, isTemporal, parseAdhoc } from '../api'
import { suggestChart } from '../suggest'
import type { Aggregation, ChartType, Dimension, Filter, FilterOp, ModelInfo, Period, ChartTile, TopN, Value } from '../api'
import { MultiSelect } from './FilterBar'
import PeriodControl from './PeriodControl'
import TileView from './TileView'

const CHARTS: { type: ChartType; label: string; icon: string }[] = [
  { type: 'table', label: 'Table', icon: '▦' },
  { type: 'bar', label: 'Bar', icon: '▮' },
  { type: 'hbar', label: 'Horizontal bar', icon: '▬' },
  { type: 'line', label: 'Line', icon: '⟋' },
  { type: 'area', label: 'Area', icon: '◭' },
  { type: 'pie', label: 'Donut', icon: '◔' },
  { type: 'scatter', label: 'Scatter', icon: '⁘' },
  { type: 'kpi', label: 'KPI', icon: '#' },
  { type: 'trend', label: 'KPI with trend', icon: '↗' },
  { type: 'combo', label: 'Bar + line', icon: '⫼' },
  { type: 'pivot', label: 'Pivot table', icon: '⊞' },
  { type: 'heatmap', label: 'Heatmap', icon: '▩' },
  { type: 'treemap', label: 'Treemap', icon: '◫' },
  { type: 'funnel', label: 'Funnel', icon: '⏷' },
  { type: 'waterfall', label: 'Waterfall', icon: '▟' },
  { type: 'histogram', label: 'Histogram', icon: '▂▅' },
  { type: 'gauge', label: 'Gauge', icon: '◠' },
]
const GRAINS = ['', 'hour', 'day', 'week', 'month', 'quarter', 'year']
const OPS: { op: FilterOp; label: string; arity: 0 | 1 | 2 | 'n' }[] = [
  { op: 'equals', label: 'is', arity: 'n' }, { op: 'not_equals', label: 'is not', arity: 'n' },
  { op: 'contains', label: 'contains', arity: 1 }, { op: 'gt', label: '>', arity: 1 }, { op: 'gte', label: '≥', arity: 1 },
  { op: 'lt', label: '<', arity: 1 }, { op: 'lte', label: '≤', arity: 1 }, { op: 'between', label: 'between', arity: 2 },
  { op: 'last_n_days', label: 'in last N days', arity: 1 }, { op: 'is_null', label: 'is empty', arity: 0 },
  { op: 'not_null', label: 'is not empty', arity: 0 },
]
const TYPE_ICON: Record<string, string> = { string: 'Aa', number: '#', boolean: '◐', date: '📅', time: '🕒', datetime: '🕘' }
const DRAG_MIME = 'application/x-magnum-field'

type Field = { kind: 'dimension' | 'measure'; name: string }

/** Aggregations that make sense for a column: any for numbers, counts for the rest. */
const aggsFor = (d?: Dimension): Aggregation[] =>
  d?.type === 'number' ? ['sum', 'avg', 'min', 'max', 'count', 'count_distinct'] : ['count_distinct', 'count']
const defaultAgg = (d: Dimension): Aggregation => d.aggregation ?? (d.type === 'number' ? 'sum' : 'count_distinct')
const adhocRef = (d: Dimension) => `${defaultAgg(d)}(${d.name})`
/** Columns that can become a number: numbers (sum…), text such as locations or ids (unique count) */
const isCountable = (d: Dimension) => d.type === 'number' || d.type === 'string' || Boolean(d.aggregation)

type Props = {
  models: ModelInfo[]
  initial?: ChartTile
  submitLabel?: string
  onSubmit?: (tile: ChartTile) => void
  onCancel?: () => void
}

export default function Explorer({ models, initial, submitLabel, onSubmit, onCancel }: Props) {
  const [modelName, setModelName] = useState(initial?.query.model ?? models[0]?.name ?? '')
  const [dims, setDims] = useState<string[]>(initial?.query.dimensions ?? [])
  const [measures, setMeasures] = useState<string[]>(initial?.query.measures ?? [])
  const [filters, setFilters] = useState<Filter[]>(initial?.query.filters ?? [])
  const [sort, setSort] = useState<string[]>(initial?.query.sort ?? [])
  const [limit, setLimit] = useState(initial?.query.limit ?? 1000)
  const [chosen, setType] = useState<ChartType>(initial?.type ?? 'table')
  const [title, setTitle] = useState(initial?.title ?? '')
  const [drill, setDrill] = useState<string>(typeof initial?.drill === 'string' ? initial.drill : '')
  // the chart was picked (or saved): no more suggestions
  const [picked, setPicked] = useState(Boolean(initial))
  const [stack, setStack] = useState<'none' | 'stacked' | 'percent'>(
    initial?.options?.percent ? 'percent' : initial?.options?.stacked ? 'stacked' : 'none')
  const [target, setTarget] = useState<number | ''>(typeof initial?.options?.target === 'number' ? initial.options.target : '')
  const [period, setPeriod] = useState<Period | null>(initial?.query.period ?? null)
  const [top, setTop] = useState<TopN | null>(initial?.query.top ?? null)
  const [search, setSearch] = useState('')
  const [over, setOver] = useState<string | null>(null)

  const model = models.find(m => m.name === modelName)
  useEffect(() => { if (!modelName && models[0]) setModelName(models[0].name) }, [models, modelName])
  const label = (ref: string) => fieldLabel(model, ref)
  // until a chart is picked, one that suits the fields chosen
  const temporal = (ref: string) => isTemporal(model?.dimensions.find(d => d.name === fieldName(ref))?.type)
  const suggested = suggestChart(dims.map(d => ({ temporal: temporal(d) })), measures.length)
  const type = picked ? chosen : suggested ?? chosen
  // Top N applies while its dimension and measure are still in the chart
  const validTop = useMemo(() => (top && dims.includes(top.dimension) && measures.includes(top.by) ? top : null),
    [top, dims, measures])

  const switchModel = (name: string) => {
    setModelName(name); setDims([]); setMeasures([]); setFilters([]); setSort([]); setDrill(''); setPeriod(null)
  }

  const add = (f: Field, shelf?: 'dimensions' | 'measures' | 'filters') => {
    const target = shelf ?? (f.kind === 'dimension' ? 'dimensions' : 'measures')
    if (target === 'filters') {
      setFilters(fs => [...fs, { field: f.name, op: 'equals', values: [] }])
    } else if (target === 'dimensions' && f.kind === 'dimension') {
      const d = model?.dimensions.find(x => x.name === f.name)
      const ref = d && isTemporal(d.type) ? `${f.name}:month` : f.name
      setDims(ds => (ds.some(x => fieldName(x) === f.name) ? ds : [...ds, ref]))
      // a hierarchy that starts at this dimension becomes the default drill path
      if (!drill && model) {
        const h = Object.entries(model.hierarchies).find(([, p]) => p[0] === f.name)
        if (h) setDrill(h[0])
      }
    } else if (target === 'measures' && f.kind === 'measure') {
      setMeasures(ms => (ms.includes(f.name) ? ms : [...ms, f.name]))
    } else if (target === 'measures' && f.kind === 'dimension') {
      // a column dropped on Measures: aggregate it (numbers: sum by default; identifiers: unique count)
      const d = model?.dimensions.find(x => x.name === f.name)
      if (d) setMeasures(ms => (ms.includes(adhocRef(d)) ? ms : [...ms, adhocRef(d)]))
    }
  }

  const onDrop = (shelf: 'dimensions' | 'measures' | 'filters') => (e: DragEvent) => {
    e.preventDefault(); setOver(null)
    const raw = e.dataTransfer.getData(DRAG_MIME)
    if (raw) add(JSON.parse(raw) as Field, shelf)
  }
  const dropProps = (shelf: 'dimensions' | 'measures' | 'filters') => ({
    onDragOver: (e: DragEvent) => { e.preventDefault(); setOver(shelf) },
    onDragLeave: () => setOver(null),
    onDrop: onDrop(shelf),
    className: `shelf-drop${over === shelf ? ' over' : ''}`,
  })

  const tile: ChartTile | null = useMemo(() => {
    if (!model || (!dims.length && !measures.length)) return null
    const sortable = [...dims, ...measures, ...(period?.compare ? measures.flatMap(m => [`${m}__previous`, `${m}__change`]) : [])]
    const validSort = sort.filter(s => sortable.includes(s.replace(/^-/, '')))
    const usable = filters.filter(f => {
      const arity = OPS.find(o => o.op === (f.op ?? 'equals'))!.arity
      const n = (f.values ?? []).length
      return arity === 0 || (arity === 'n' ? n > 0 : n >= arity)
    })
    return {
      id: initial?.id ?? 'preview',
      title: title || null,
      type,
      drill: drill || null,
      layout: initial?.layout ?? { x: 0, y: 0, w: 6, h: 5 },
      options: {
        ...(stack !== 'none' && ['bar', 'area', 'line'].includes(type) ? { stacked: true } : {}),
        ...(stack === 'percent' && ['bar', 'area'].includes(type) ? { percent: true } : {}),
        ...(type === 'gauge' && target !== '' ? { target } : {}),
      },
      query: {
        model: model.name, dimensions: dims, measures,
        filters: usable, sort: validSort.length ? validSort : undefined, limit,
        ...(period ? { period } : {}),
        ...(validTop ? { top: validTop } : {}),
      },
    }
  }, [model, dims, measures, filters, sort, limit, type, title, drill, stack, target, period, initial, validTop])

  const q = search.toLowerCase()
  const match = (n: string, l?: string | null) => !q || n.toLowerCase().includes(q) || (l ?? '').toLowerCase().includes(q)

  return (
    <div className="explorer">
      <aside className="fields">
        <select value={modelName} onChange={e => switchModel(e.target.value)}>
          {models.map(m => <option key={m.name} value={m.name}>{m.label}</option>)}
        </select>
        {model?.description && <p className="muted small">{model.description}</p>}
        <input placeholder="Search fields…" value={search} onChange={e => setSearch(e.target.value)} />
        <div className="field-group">Dimensions</div>
        {model?.dimensions.filter(d => match(d.name, d.label)).map(d => (
          <FieldItem key={d.name} field={{ kind: 'dimension', name: d.name }} label={fieldLabel(model, d.name)}
            icon={TYPE_ICON[d.type]} title={d.description ?? undefined} onAdd={add} used={dims.some(x => fieldName(x) === d.name)}
            extra={isCountable(d) ? (
              <button className="add" title={`Add as a number (${AGG_LABELS[defaultAgg(d)]}); the aggregation can be changed`}
                onClick={() => setMeasures(ms => (ms.includes(adhocRef(d)) ? ms : [...ms, adhocRef(d)]))}>Σ</button>
            ) : undefined} />
        ))}
        <div className="field-group">Measures</div>
        {model?.measures.filter(m => match(m.name, m.label)).map(m => (
          <FieldItem key={m.name} field={{ kind: 'measure', name: m.name }} label={fieldLabel(model, m.name)}
            icon="Σ" title={m.description ?? undefined} onAdd={add} used={measures.includes(m.name)} />
        ))}
        {model && Object.keys(model.hierarchies).length > 0 && <>
          <div className="field-group">Drill paths</div>
          {Object.entries(model.hierarchies).map(([h, p]) => (
            <div key={h} className="muted small hier"><b>{h}</b>: {p.map(x => fieldLabel(model, x)).join(' › ')}</div>
          ))}
        </>}
      </aside>

      <section className="builder">
        <div className="shelves">
          <Shelf label="Dimensions" drop={dropProps('dimensions')} empty="Drag dimensions here">
            {dims.map((ref, i) => {
              const d = model?.dimensions.find(x => x.name === fieldName(ref))
              const temporal = isTemporal(d?.type)
              return (
                <Chip key={ref} kind="dimension" label={label(fieldName(ref))} onRemove={() => setDims(dims.filter((_, j) => j !== i))}>
                  {temporal && (
                    <select value={fieldGrain(ref) ?? ''} onChange={e => {
                      const next = e.target.value ? `${fieldName(ref)}:${e.target.value}` : fieldName(ref)
                      setDims(dims.map((x, j) => (j === i ? next : x)))
                    }}>
                      {GRAINS.filter(g => g !== 'hour' || d?.type !== 'date').map(g => <option key={g} value={g}>{g || 'raw'}</option>)}
                    </select>
                  )}
                </Chip>
              )
            })}
          </Shelf>
          <Shelf label="Measures" drop={dropProps('measures')} empty="Drag measures here">
            {measures.map((m, i) => {
              const adhoc = parseAdhoc(m)
              const d = adhoc ? model?.dimensions.find(x => x.name === adhoc.field) : undefined
              return (
                <Chip key={m} kind="measure" label={adhoc ? fieldLabel(model, adhoc.field) : label(m)}
                  onRemove={() => setMeasures(measures.filter(x => x !== m))}>
                  {adhoc && (
                    <select title="Aggregation" value={adhoc.agg} onChange={e => {
                      const next = `${e.target.value}(${adhoc.field})`
                      setMeasures(measures.map((x, j) => (j === i ? next : x)))
                      setSort(ss => ss.map(s => s.replace(m, next)))
                      setTop(t => (t && t.by === m ? { ...t, by: next } : t))
                    }}>
                      {aggsFor(d).map(a => <option key={a} value={a}>{AGG_LABELS[a]}</option>)}
                    </select>
                  )}
                </Chip>
              )
            })}
          </Shelf>
          <Shelf label="Filters" drop={dropProps('filters')} empty="Drag any field here to filter">
            {filters.map((f, i) => (
              <FilterChip key={i} model={model!} filter={f}
                onChange={nf => setFilters(filters.map((x, j) => (j === i ? nf : x)))}
                onRemove={() => setFilters(filters.filter((_, j) => j !== i))} />
            ))}
          </Shelf>
        </div>

        <div className="viz-bar">
          <div className="chart-types">
            {CHARTS.map(c => (
              <button key={c.type} className={type === c.type ? 'active' : ''} title={c.label} onClick={() => {
                setType(c.type); setPicked(true)
                // a histogram counts every value: the most rows a query may have
                if (c.type === 'histogram' && limit === 1000) setLimit(50000)
              }}>
                <span>{c.icon}</span> {c.label}
              </button>
            ))}
          </div>
          <div className="viz-opts">
            <label>Sort
              <select value={sort[0] ?? ''} onChange={e => setSort(e.target.value ? [e.target.value] : [])}>
                <option value="">auto</option>
                {[...dims, ...measures].flatMap(f => [
                  <option key={f} value={f}>{label(f)} ↑</option>,
                  <option key={'-' + f} value={'-' + f}>{label(f)} ↓</option>,
                ])}
                {period?.compare && measures.flatMap(m => [
                  <option key={'-' + m + '__change'} value={'-' + m + '__change'}>{fieldLabel(model, m)} Δ% ↓ (risers)</option>,
                  <option key={m + '__change'} value={m + '__change'}>{fieldLabel(model, m)} Δ% ↑ (fallers)</option>,
                ])}
              </select>
            </label>
            <TopControl top={top} dims={dims} measures={measures} label={label}
              onChange={t => { setTop(t); if (t) setSort([]) }} />
            {!validTop && <label>Limit <input type="number" min={1} max={50000} value={limit} onChange={e => setLimit(Math.max(1, Number(e.target.value) || 1))} /></label>}
            {model && Object.keys(model.hierarchies).length > 0 && (
              <label>Drill
                <select value={drill} onChange={e => setDrill(e.target.value)}>
                  <option value="">none</option>
                  {Object.keys(model.hierarchies).map(h => <option key={h} value={h}>{h}</option>)}
                </select>
              </label>
            )}
            {['bar', 'area', 'line'].includes(type) && (
              <label>Stack
                <select value={stack} onChange={e => setStack(e.target.value as typeof stack)}>
                  <option value="none">no</option><option value="stacked">stacked</option>
                  {type !== 'line' && <option value="percent">100%</option>}
                </select>
              </label>
            )}
            {type === 'gauge' && <label>Target <input type="number" min={0} value={target} placeholder="none"
              onChange={e => setTarget(e.target.value === '' ? '' : Math.max(0, Number(e.target.value)))} /></label>}
          </div>
        </div>

        <PeriodControl model={model} period={period} onChange={setPeriod} editableField />

        <div className="preview">
          {tile ? <TileView key={JSON.stringify(tile.query) + tile.drill} tile={tile} model={model} />
            : <div className="empty big">Drag a dimension and a measure from the left to start exploring.</div>}
        </div>

        {onSubmit && (
          <div className="builder-foot">
            <input className="title-input" placeholder="Tile title (optional)" value={title} onChange={e => setTitle(e.target.value)} />
            {onCancel && <button onClick={onCancel}>Cancel</button>}
            <button className="primary" disabled={!tile} onClick={() => tile && onSubmit(tile)}>{submitLabel ?? 'Save'}</button>
          </div>
        )}
      </section>
    </div>
  )
}

/** Show: all rows · top N · bottom N, of a dimension, by a measure */
function TopControl({ top, dims, measures, label, onChange }: {
  top: TopN | null; dims: string[]; measures: string[]; label: (ref: string) => string; onChange: (t: TopN | null) => void
}) {
  if (!dims.length || !measures.length) return null
  const mode = top ? (top.bottom ? 'bottom' : 'top') : 'all'
  const dim = top && dims.includes(top.dimension) ? top.dimension : dims.find(d => !d.includes(':')) ?? dims[0]
  const by = top && measures.includes(top.by) ? top.by : measures[0]
  const set = (patch: Partial<TopN> & { mode?: string }) => {
    const m = patch.mode ?? mode
    if (m === 'all') { onChange(null); return }
    onChange({ dimension: patch.dimension ?? dim, n: patch.n ?? top?.n ?? 10, by: patch.by ?? by, bottom: m === 'bottom' })
  }
  return (
    <span className="top-control">
      <label>Show
        <select value={mode} onChange={e => set({ mode: e.target.value })}>
          <option value="all">all</option><option value="top">top</option><option value="bottom">bottom</option>
        </select>
      </label>
      {top && <>
        <input type="number" min={1} max={1000} value={top.n} aria-label="How many"
          onChange={e => set({ n: Math.min(1000, Math.max(1, Number(e.target.value) || 1)) })} />
        {dims.length > 1
          ? <select value={dim} aria-label="Of" onChange={e => set({ dimension: e.target.value })}>
              {dims.map(d => <option key={d} value={d}>{label(d)}</option>)}</select>
          : <span className="muted">{label(dim)}</span>}
        <span className="muted">by</span>
        <select value={by} aria-label="By" onChange={e => set({ by: e.target.value })}>
          {measures.map(m => <option key={m} value={m}>{label(m)}</option>)}
        </select>
      </>}
    </span>
  )
}

function FieldItem({ field, label, icon, title, onAdd, used, extra }: { field: Field; label: string; icon: string; title?: string; onAdd: (f: Field) => void; used: boolean; extra?: ReactNode }) {
  return (
    <div className={`field-item ${field.kind}${used ? ' used' : ''}`} draggable title={title ?? 'Drag to a shelf, or double-click to add'}
      onDragStart={e => { e.dataTransfer.setData(DRAG_MIME, JSON.stringify(field)); e.dataTransfer.effectAllowed = 'copy' }}
      onDoubleClick={() => onAdd(field)}>
      <span className="ficon">{icon}</span><span className="flabel">{label}</span>
      {extra}
      <button className="add" onClick={() => onAdd(field)} title="Add">+</button>
    </div>
  )
}

function Shelf({ label, drop, empty, children }: { label: string; drop: object; empty: string; children: ReactNode }) {
  const has = Array.isArray(children) ? children.length > 0 : Boolean(children)
  return (
    <div className="shelf">
      <div className="shelf-label">{label}</div>
      <div {...drop}>{has ? children : <span className="placeholder">{empty}</span>}</div>
    </div>
  )
}

function Chip({ kind, label, onRemove, children }: { kind: string; label: string; onRemove: () => void; children?: ReactNode }) {
  return (
    <span className={`chip ${kind}`}>
      {label}{children}<button onClick={onRemove} title="Remove">×</button>
    </span>
  )
}

function FilterChip({ model, filter, onChange, onRemove }: { model: ModelInfo; filter: Filter; onChange: (f: Filter) => void; onRemove: () => void }) {
  const name = fieldName(filter.field)
  const dim = model.dimensions.find(d => d.name === name)
  const op = filter.op ?? 'equals'
  const arity = OPS.find(o => o.op === op)!.arity
  const parse = (s: string): Value => (dim?.type === 'number' || !dim) && s !== '' && !isNaN(Number(s)) ? Number(s) : s
  const values = filter.values ?? []
  return (
    <span className="chip flt">
      {fieldLabel(model, name)}
      <select value={op} onChange={e => onChange({ ...filter, op: e.target.value as FilterOp, values: [] })}>
        {OPS.filter(o => (o.op === 'last_n_days' ? isTemporal(dim?.type) : o.op !== 'contains' || dim?.type === 'string'))
          .map(o => <option key={o.op} value={o.op}>{o.label}</option>)}
      </select>
      {arity === 'n' && dim?.type === 'string' ? (
        <MultiSelect model={model.name} field={name} values={values} onChange={v => onChange({ ...filter, values: v })} />
      ) : arity === 'n' && dim?.type === 'boolean' ? (
        <select value={values.length ? String(values[0]) : ''} onChange={e => onChange({ ...filter, values: e.target.value ? [e.target.value === 'true'] : [] })}>
          <option value="">–</option><option value="true">Yes</option><option value="false">No</option>
        </select>
      ) : arity !== 0 && (
        <input className="fval" placeholder={arity === 2 ? 'from, to' : arity === 'n' ? 'a, b, c' : 'value'}
          defaultValue={values.map(String).join(', ')}
          onBlur={e => onChange({ ...filter, values: e.target.value.split(',').map(s => s.trim()).filter(Boolean).map(parse) })}
          onKeyDown={e => { if (e.key === 'Enter') (e.target as HTMLInputElement).blur() }} />
      )}
      <button onClick={onRemove} title="Remove">×</button>
    </span>
  )
}
