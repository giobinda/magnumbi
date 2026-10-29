import { useEffect, useRef, useState } from 'react'
import { api, fieldLabel, isTemporal } from '../api'
import type { Dashboard, DashboardFilter, FilterOp, ModelInfo, Value } from '../api'
import { FILTER_OPS, filterChoices, flippable, linkChoices, newFilter, opOf, withLink } from '../dashboardFilters'

export const filterKey = (f: DashboardFilter) => `${f.model}.${f.field}`

type Props = {
  /** saved dashboard: pick lists come from its declared filters (works for every role) */
  dashboardId?: string
  filters: DashboardFilter[]
  models: ModelInfo[]
  values: Record<string, Value[]>
  onChange: (key: string, values: Value[]) => void
  /** the viewer's is / is not */
  ops?: Record<string, FilterOp>
  onOp?: (key: string, op: FilterOp) => void
  /** editing the dashboard: filters to add (on its models' fields) and remove */
  editing?: { dashboard: Dashboard; onAdd: (f: DashboardFilter) => void; onRemove: (key: string) => void }
}

export default function FilterBar({ dashboardId, filters, models, values, onChange, ops = {}, onOp, editing }: Props) {
  if (!filters.length && !editing) return null
  return (
    <div className="filter-bar" data-tour="filters">
      {filters.map(f => {
        const model = models.find(m => m.name === f.model)
        const dim = model?.dimensions.find(d => d.name === f.field)
        const key = filterKey(f)
        const label = f.label || fieldLabel(model, f.field)
        const v = values[key] ?? []
        const op = opOf(f, ops)
        return (
          <label key={key} className="filter">
            <span>{label}
              {editing && <button type="button" className="link filter-remove" title="Remove this filter" onClick={e => { e.preventDefault(); editing.onRemove(key) }}>×</button>}
            </span>
            <span className="filter-input">
            {flippable(f) && dim && !isTemporal(dim.type) && dim.type !== 'boolean' && onOp && (
              <select className="filter-op" aria-label={`${label}: is or is not`} value={op} onChange={e => onOp(key, e.target.value as FilterOp)}>
                <option value="equals">is</option><option value="not_equals">is not</option>
              </select>
            )}
            {op === 'is_null' || op === 'not_null' ? (
              <span className="muted small filter-fixed">{op === 'is_null' ? 'is empty' : 'is not empty'}</span>
            ) : op === 'last_n_days' ? (
              <select value={String(v[0] ?? '')} onChange={e => onChange(key, e.target.value ? [Number(e.target.value)] : [])}>
                <option value="">All time</option>
                {[7, 30, 90, 180, 365, 730].map(n => <option key={n} value={n}>Last {n} days</option>)}
                {v[0] !== undefined && ![7, 30, 90, 180, 365, 730].includes(Number(v[0])) && <option value={String(v[0])}>Last {String(v[0])} days</option>}
              </select>
            ) : dim?.type === 'boolean' ? (
              <select value={v.length ? String(v[0]) : ''} onChange={e => onChange(key, e.target.value ? [e.target.value === 'true'] : [])}>
                <option value="">Any</option><option value="true">Yes</option><option value="false">No</option>
              </select>
            ) : (op === 'equals' || op === 'not_equals') && dim && !isTemporal(dim.type) ? (
              <MultiSelect model={f.model} field={f.field} values={v} onChange={vals => onChange(key, vals)}
                load={dashboardId ? () => api.filterValues(dashboardId, key).then(r => r.rows.map(x => x[0])) : undefined} />
            ) : (
              <input value={v.map(String).join(',')} placeholder={op === 'between' ? 'from,to' : 'value'}
                onChange={e => onChange(key, e.target.value ? e.target.value.split(',').map(s => s.trim()) : [])} />
            )}
            </span>
          </label>
        )
      })}
      {editing && <AddFilter dashboard={editing.dashboard} models={models} onAdd={editing.onAdd} />}
    </div>
  )
}

/** Editing a dashboard: a filter on one of its models' fields, and how it filters. */
function AddFilter({ dashboard, models, onAdd }: { dashboard: Dashboard; models: ModelInfo[]; onAdd: (f: DashboardFilter) => void }) {
  const [open, setOpen] = useState(false)
  const [field, setField] = useState('')
  const [op, setOp] = useState<FilterOp>('equals')
  const [label, setLabel] = useState('')
  // the same filter on the dashboard's other models: their field (suggested when the field is picked)
  const [links, setLinks] = useState<Record<string, string>>({})
  const taken = dashboard.filters.map(filterKey)
  const choices = filterChoices(dashboard, models).filter(c => !taken.includes(`${c.model}.${c.field}`))
  if (!open) return <button type="button" className="link add-filter" onClick={() => setOpen(true)}>+ Add a filter</button>
  const add = () => {
    const c = choices.find(x => `${x.model}.${x.field}` === field)
    if (!c) return
    onAdd(Object.entries(links).reduce((f, [m, x]) => withLink(f, m, x), newFilter(c.model, c.field, op, label)))
    setOpen(false); setField(''); setOp('equals'); setLabel(''); setLinks({})
  }
  const picked = choices.find(x => `${x.model}.${x.field}` === field)
  const pick = (key: string) => {
    setField(key)
    const c = choices.find(x => `${x.model}.${x.field}` === key)
    setLinks(c ? Object.fromEntries(linkChoices(dashboard, models, c.model, c.field).filter(l => l.suggested).map(l => [l.model, l.suggested])) : {})
  }
  return (
    <div className="add-filter-form" role="group" aria-label="Add a filter">
      <select value={field} onChange={e => pick(e.target.value)} aria-label="Field">
        <option value="">Field…</option>
        {choices.map(c => <option key={`${c.model}.${c.field}`} value={`${c.model}.${c.field}`}>{c.label}</option>)}
      </select>
      <select value={op} onChange={e => setOp(e.target.value as FilterOp)} aria-label="How it filters">
        {FILTER_OPS.map(o => <option key={o.op} value={o.op}>{o.label}</option>)}
      </select>
      {picked && linkChoices(dashboard, models, picked.model, picked.field).map(l => (
        <select key={l.model} value={links[l.model] ?? ''} aria-label={`On ${l.label}`} title={`The same filter on ${l.label}'s tiles`}
          onChange={e => setLinks(s => ({ ...s, [l.model]: e.target.value }))}>
          <option value="">Not on {l.label}</option>
          {l.options.map(o => <option key={o.field} value={o.field}>On {o.label}</option>)}
        </select>
      ))}
      <input value={label} placeholder="Label (optional)" onChange={e => setLabel(e.target.value)} />
      <button type="button" className="primary" disabled={!field} onClick={add}>Add</button>
      <button type="button" onClick={() => setOpen(false)}>Cancel</button>
    </div>
  )
}

export function MultiSelect({ model, field, values, onChange, load }: {
  model: string; field: string; values: Value[]; onChange: (v: Value[]) => void; load?: () => Promise<Value[]>
}) {
  const [open, setOpen] = useState(false)
  const [options, setOptions] = useState<Value[] | null>(null)
  const [search, setSearch] = useState('')
  const [err, setErr] = useState<string | null>(null)
  const ref = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (!open || options) return
    ;(load ? load() : api.query({ model, dimensions: [field], sort: [field], limit: 500 }).then(r => r.rows.map(x => x[0])))
      .then(setOptions, e => setErr(e.message))
  }, [open, options, model, field, load])

  useEffect(() => {
    if (!open) return
    const close = (e: MouseEvent) => { if (!ref.current?.contains(e.target as Node)) setOpen(false) }
    document.addEventListener('mousedown', close)
    return () => document.removeEventListener('mousedown', close)
  }, [open])

  const toggle = (o: Value) => onChange(values.includes(o) ? values.filter(x => x !== o) : [...values, o])
  const summary = values.length === 0 ? 'All' : values.length <= 2 ? values.map(v => v ?? '∅').join(', ') : `${values.length} selected`
  const shown = (options ?? []).filter(o => String(o ?? '∅').toLowerCase().includes(search.toLowerCase()))

  return (
    <div className="multiselect" ref={ref}>
      <button type="button" className="ms-btn" onClick={() => setOpen(o => !o)}>{summary} ▾</button>
      {open && (
        <div className="ms-pop">
          <input autoFocus placeholder="Search…" value={search} onChange={e => setSearch(e.target.value)} />
          <div className="ms-actions">
            <button className="link" onClick={() => onChange([])}>Clear</button>
            {options && options.length > 0 && <button className="link" onClick={() => onChange(shown)}>Select shown</button>}
          </div>
          <div className="ms-list">
            {err ? <div className="error">{err}</div>
              : !options ? <div className="empty">Loading…</div>
              : shown.map(o => (
                <label key={String(o)} className="ms-opt">
                  <input type="checkbox" checked={values.includes(o)} onChange={() => toggle(o)} />
                  {o === null ? '∅ (empty)' : String(o)}
                </label>
              ))}
          </div>
        </div>
      )}
    </div>
  )
}
