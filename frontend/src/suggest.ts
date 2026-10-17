import type { ChartType } from './api'

/** A chart for the fields chosen in Explore (until the person picks one): `dims` in order
 *  (temporal: a date or time), `measures` how many. Null: nothing chosen yet. */
export function suggestChart(dims: { temporal: boolean }[], measures: number): ChartType | null {
  if (!dims.length && !measures) return null
  if (!measures || dims.length > 2) return 'table'
  if (!dims.length) return 'kpi'
  const dated = dims.some(d => d.temporal)
  if (dims.length === 1) return dated ? (measures === 2 ? 'combo' : 'line') : measures <= 2 ? 'bar' : 'table'
  if (measures > 1) return 'table'
  return dated ? 'line' : 'heatmap'
}
