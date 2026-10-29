import type { Column, Filter, Query, Value } from './api'
import { fieldName, parseIso } from './api'
import { formatValue } from './format'

/** One drill step: swap a dimension for a finer one and filter on the clicked value. */
export interface DrillStep { from: string; to: string; filters: Filter[]; label: string; click: { field: string; value: Value } }

const FINER: Record<string, string> = { year: 'quarter', quarter: 'month', month: 'day', week: 'day' }

/** Whether clicking the chart drills: a field shown has a level below it on the drill path, or
 *  is a date at a grain with a finer one (not the field the period owns). */
export function drillable(dims: string[], path: string[], periodField?: string): boolean {
  return dims.some(d => {
    const [name, grain] = d.split(':')
    const i = path.indexOf(name)
    return (i >= 0 && i + 1 < path.length) || (Boolean(grain && FINER[grain]) && name !== periodField)
  })
}

export function applyDrill(q: Query, steps: DrillStep[]): Query {
  let dims = [...(q.dimensions ?? [])]
  let filters = [...(q.filters ?? [])]
  let sort = q.sort
  for (const s of steps) {
    dims = dims.map(d => (d === s.from ? s.to : d))
    filters = [...filters, ...s.filters]
    // a sort on the replaced dimension no longer applies
    sort = sort?.filter(x => x.replace(/^[-+]/, '') !== s.from)
  }
  return { ...q, dimensions: dims, filters, sort }
}

/** What clicking `value` in `column` drills into, or null if it isn't drillable. */
export function nextDrill(column: Column, value: Value, path: string[], periodField?: string): DrillStep | null {
  if (column.kind !== 'dimension') return null
  const name = fieldName(column.field)
  const i = path.indexOf(name)
  if (i >= 0 && i + 1 < path.length) {
    return { from: column.field, to: path[i + 1], filters: [{ field: name, op: 'equals', values: [value] }], label: formatValue(value, column),
      click: { field: column.field, value } }
  }
  // time dimensions drill to a finer grain within the clicked period (unless a period
  // comparison owns that field's window)
  if (column.grain && FINER[column.grain] && typeof value === 'string' && name !== periodField) {
    const [start, end] = period(value, column.grain)
    return {
      from: column.field, to: `${name}:${FINER[column.grain]}`,
      filters: [{ field: name, op: 'gte', values: [start] }, { field: name, op: 'lt', values: [end] }],
      label: formatValue(value, column),
      click: { field: column.field, value },
    }
  }
  return null
}

function period(iso: string, grain: string): [string, string] {
  const d = parseIso(iso)
  const y = d.getUTCFullYear(), m = d.getUTCMonth(), day = d.getUTCDate()
  const end = grain === 'year' ? new Date(Date.UTC(y + 1, 0, 1))
    : grain === 'quarter' ? new Date(Date.UTC(y, m + 3, 1))
    : grain === 'month' ? new Date(Date.UTC(y, m + 1, 1))
    : new Date(Date.UTC(y, m, day + 7))
  const fmt = (x: Date) => x.toISOString().slice(0, 19).replace('T', ' ') + '+00'
  return [fmt(d), fmt(end)]
}
