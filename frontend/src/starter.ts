import { isTemporal } from './api'
import type { Dashboard, ModelInfo, Tile } from './api'

/** A first dashboard for a model, to change at will: its main numbers (and their trend when it
 *  has a date), activity over time, the top categories, and the details. */
export function starterDashboard(m: ModelInfo): Dashboard {
  const [first, ...rest] = m.measures
  const second = rest.find(x => x.type !== 'count' && x.type !== 'avg') ?? rest[0]
  const date = m.dimensions.find(d => isTemporal(d.type))
  const day = date ? `${date.name}:day` : null
  const [cat1, cat2] = m.dimensions.filter(d => d.type === 'string')
  const label = (x: { name: string; label?: string | null }) => x.label ?? x.name
  const tiles: Tile[] = []
  const add = (t: Omit<Tile, 'query'> & { query: Partial<Tile['query']> }) => tiles.push({ ...t, query: { model: m.name, ...t.query } as Tile['query'] })
  if (!first) return { title: `${m.label ?? m.name} overview`, filters: [], tiles }

  // the numbers
  add({ id: 'total', title: label(first), type: 'kpi', query: { measures: [first.name] }, layout: { x: 0, y: 0, w: 3, h: 2 } })
  if (second) add({ id: 'second', title: label(second), type: 'kpi', query: { measures: [second.name] }, layout: { x: 3, y: 0, w: 3, h: 2 } })
  if (day) add({ id: 'trend', title: `${label(first)}, latest day`, type: 'trend', query: { dimensions: [day], measures: [first.name] }, layout: { x: 6, y: 0, w: 6, h: 2 } })
  // over time, and the top categories
  if (day) add({ id: 'over_time', title: `${label(first)} per day`, type: 'line', query: { dimensions: [day], measures: [first.name] }, layout: { x: 0, y: 2, w: 7, h: 5 } })
  if (cat1) add({ id: 'top', title: `Top ${label(cat1)}`, type: 'hbar', query: { dimensions: [cat1.name], measures: [first.name], sort: [`-${first.name}`], limit: 10 },
    layout: day ? { x: 7, y: 2, w: 5, h: 5 } : { x: 0, y: 2, w: 12, h: 5 } })
  // shares, and the details
  if (cat2) add({ id: 'shares', title: `${label(first)} by ${label(cat2)}`, type: 'pie', query: { dimensions: [cat2.name], measures: [first.name], sort: [`-${first.name}`], limit: 8 }, layout: { x: 0, y: 7, w: 5, h: 5 } })
  const cats = [cat1, cat2].filter(Boolean).map(d => d!.name)
  if (cats.length) add({ id: 'details', title: 'Details', type: 'table', query: { dimensions: cats, measures: [first.name, ...(second ? [second.name] : [])], sort: [`-${first.name}`], limit: 100 },
    layout: cat2 ? { x: 5, y: 7, w: 7, h: 5 } : { x: 0, y: 7, w: 12, h: 5 } })
  return {
    title: `${m.label ?? m.name} overview`,
    description: 'A starter dashboard: change anything with Edit.',
    filters: cats.map(field => ({ model: m.name, field })),
    tiles,
  }
}
