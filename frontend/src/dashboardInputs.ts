import { useMemo, useState } from 'react'
import type { Dashboard, DashboardPeriod, Filter, Value } from './api'
import { filterKey } from './components/FilterBar'

/** A dashboard's filter values as it opens: URL params (?status=Complete,Shipped) override the
 *  defaults. */
export function initialFilterValues(d: Dashboard): Record<string, Value[]> {
  const params = new URLSearchParams(location.search)
  const out: Record<string, Value[]> = {}
  for (const f of d.filters) {
    const p = params.get(f.field)
    out[filterKey(f)] = p !== null ? (p === '' ? [] : p.split(',')) : (f.default ?? [])
  }
  return out
}

/** What a viewer picks on a dashboard (filter values, period), and what that means for its
 *  tiles: extra filters per model, the period, and the request a saved tile is run with. */
export function useDashboardInputs(dash: Dashboard | null) {
  const [filterValues, setFilterValues] = useState<Record<string, Value[]>>({})
  const [period, setPeriod] = useState<DashboardPeriod | null>(null)  // the viewer's current choice

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

  return { filterValues, setFilterValues, period, setPeriod, filtersByModel, tilePeriod, tileRequest }
}
