import { useEffect, useRef, useState } from 'react'
import { api, fieldLabel, isTemporal } from '../api'
import type { DashboardFilter, ModelInfo, Value } from '../api'

export const filterKey = (f: DashboardFilter) => `${f.model}.${f.field}`

type Props = {
  /** saved dashboard: pick lists come from its declared filters (works for every role) */
  dashboardId?: string
  filters: DashboardFilter[]
  models: ModelInfo[]
  values: Record<string, Value[]>
  onChange: (key: string, values: Value[]) => void
}

export default function FilterBar({ dashboardId, filters, models, values, onChange }: Props) {
  if (!filters.length) return null
  return (
    <div className="filter-bar">
      {filters.map(f => {
        const model = models.find(m => m.name === f.model)
        const dim = model?.dimensions.find(d => d.name === f.field)
        const key = filterKey(f)
        const label = f.label || fieldLabel(model, f.field)
        const v = values[key] ?? []
        const op = f.op ?? 'equals'
        return (
          <label key={key} className="filter">
            <span>{label}{op === 'not_equals' ? ' (exclude)' : ''}</span>
            {op === 'last_n_days' ? (
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
          </label>
        )
      })}
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
