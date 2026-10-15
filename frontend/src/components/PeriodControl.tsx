import { useEffect, useState } from 'react'
import { api, isTemporal } from '../api'
import type { ModelInfo, Period, ResolvedPeriod } from '../api'
import { COMPARE_OPTIONS, RANGE_OPTIONS, compareFromKey, compareKey, rangeFromKey, rangeKey, windowLabel } from '../period'

type Props = {
  model?: ModelInfo
  period: Period | null
  onChange: (p: Period | null) => void
  /** let the user pick the date field and turn the period off (explorer) */
  editableField?: boolean
}

/** Period picker: current window + optional comparison window, with resolved dates. */
export default function PeriodControl({ model, period, onChange, editableField }: Props) {
  const timeDims = model?.dimensions.filter(d => isTemporal(d.type)) ?? []
  const [resolved, setResolved] = useState<ResolvedPeriod | null>(null)
  const key = period && model ? JSON.stringify({ field: period.field, current: period.current, compare: period.compare ?? null }) : ''

  useEffect(() => {
    if (!key || !model) { setResolved(null); return }
    let live = true
    api.resolvePeriod(model.name, JSON.parse(key)).then(r => live && setResolved(r), () => live && setResolved(null))
    return () => { live = false }
  }, [key, model])

  if (!timeDims.length) return null
  const set = (patch: Partial<Period>) => period && onChange({ ...period, ...patch })

  return (
    <div className="period">
      {editableField && (
        <label>Period on
          <select value={period?.field ?? ''} onChange={e => onChange(e.target.value
            ? { field: e.target.value, current: period?.current ?? { last_n_days: 30 }, compare: period?.compare ?? null }
            : null)}>
            <option value="">— none —</option>
            {timeDims.map(d => <option key={d.name} value={d.name}>{d.label || d.name}</option>)}
          </select>
        </label>
      )}
      {period && <>
        <label>{editableField ? 'Range' : 'Period'}
          <select value={rangeKey(period.current)} onChange={e => set({ current: rangeFromKey(e.target.value, period.current) })}>
            {RANGE_OPTIONS.map(o => <option key={o.value} value={o.value}>{o.label}</option>)}
          </select>
        </label>
        {rangeKey(period.current) === 'custom' && <>
          <input type="date" value={period.current.from ?? ''} onChange={e => e.target.value && set({ current: { ...period.current, from: e.target.value } })} />
          <span className="muted">to</span>
          <input type="date" value={period.current.to ?? ''} onChange={e => set({ current: { ...period.current, to: e.target.value || undefined } })} />
        </>}
        <label>Compare to
          <select value={compareKey(period.compare)} onChange={e => set({ compare: compareFromKey(e.target.value, period.compare) })}>
            {COMPARE_OPTIONS.map(o => <option key={o.value} value={o.value}>{o.label}</option>)}
          </select>
        </label>
        {typeof period.compare === 'object' && period.compare && (
          <input type="date" title="Comparison starts on" value={period.compare.from}
            onChange={e => e.target.value && set({ compare: { from: e.target.value } })} />
        )}
        {resolved && (
          <span className="period-range" title={resolved.previous ? `Previous window shifted back ${resolved.offset_days} days` : undefined}>
            {windowLabel(resolved.current)}{resolved.previous && <> <span className="muted">vs</span> {windowLabel(resolved.previous)}</>}
          </span>
        )}
      </>}
    </div>
  )
}
