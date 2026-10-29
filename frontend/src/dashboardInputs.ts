import { useMemo, useState } from 'react'
import type { Dashboard, DashboardPeriod, FilterOp, Value } from './api'
import { filterKey } from './components/FilterBar'
import { filtersFor } from './dashboardFilters'

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
  // the viewer's is / is not, per filter
  const [filterOps, setFilterOps] = useState<Record<string, FilterOp>>({})

  const filtersByModel = useMemo(() => (dash ? filtersFor(dash, filterValues, filterOps) : {}), [dash, filterValues, filterOps])

  // the query-level period has no `model`; it only applies to tiles on that model
  const tilePeriod = useMemo(() => {
    if (!period) return null
    const { model: _model, ...p } = period
    return p
  }, [period])

  // what a saved tile may be asked for: filter values and the period window (drill is per tile)
  const tileRequest = useMemo(() => ({
    filters: filterValues,
    ops: filterOps,
    period: period ? { current: period.current, compare: period.compare ?? null } : null,
  }), [filterValues, filterOps, period])

  return { filterValues, setFilterValues, filterOps, setFilterOps, period, setPeriod, filtersByModel, tilePeriod, tileRequest }
}
