// A dashboard's own filters: which fields they may be on, how each filters (the builder's choice,
// and the viewer's is / is not), and what they give each model's tiles.
import { isTemporal } from './api'
import type { Dashboard, DashboardFilter, DashboardPeriod, Filter, FilterOp, ModelInfo, Period, Value } from './api'
import { filterKey } from './components/FilterBar'

/** How a filter may filter, in the editor: is, is not, contains, empty or not. */
export const FILTER_OPS: { op: FilterOp; label: string }[] = [
  { op: 'equals', label: 'is' }, { op: 'not_equals', label: 'is not' }, { op: 'contains', label: 'contains' },
  { op: 'not_null', label: 'is not empty' }, { op: 'is_null', label: 'is empty' },
]

const FLIPS: FilterOp[] = ['equals', 'not_equals']

/** A filter a viewer may turn from "is" to "is not" (and back). */
export const flippable = (f: DashboardFilter) => FLIPS.includes(f.op ?? 'equals')

/** How the filter filters now: the viewer's is / is not, where it may be flipped. */
export function opOf(f: DashboardFilter, ops: Record<string, FilterOp>): FilterOp {
  const chosen = ops[filterKey(f)]
  return chosen && flippable(f) && FLIPS.includes(chosen) ? chosen : f.op ?? 'equals'
}

/** Each model's filters from the dashboard's, as the viewer set them. */
export function filtersFor(dash: Dashboard, values: Record<string, Value[]>, ops: Record<string, FilterOp>): Record<string, Filter[]> {
  const out: Record<string, Filter[]> = {}
  for (const f of dash.filters) {
    const v = values[filterKey(f)] ?? []
    const op = opOf(f, ops)
    if (!v.length && op !== 'is_null' && op !== 'not_null') continue
    // on its model, and on the others it names a field of
    for (const [model, field] of [[f.model, f.field], ...Object.entries(f.fields ?? {})]) {
      ;(out[model] ??= []).push({ field, op, values: v })
    }
  }
  return out
}

/** The dashboard's period for a tile on `model`: on its own date there if it reaches it; on a
 *  blend of the model, under the blend's name for it (taxi.pickup_at). */
export function periodFor(p: DashboardPeriod | null, model: ModelInfo | undefined): Period | null {
  if (!p || !model) return null
  const reached: [string, string][] = [[p.model, p.field], ...Object.entries(p.fields ?? {})]
  const parts = model.blend ? [model.blend.base, ...model.blend.joins.map(j => j.model)] : []
  const field = reached.find(([m]) => m === model.name)?.[1]
    ?? reached.filter(([m]) => parts.includes(m)).map(([m, f]) => `${m}.${f}`)[0]
  if (!field) return null
  const { model: _model, fields: _fields, ...rest } = p
  return { ...rest, field }
}

/** The dashboard's filters for a tile on `model`: its own, or on a blend its models', under the
 *  blend's names (taxi.pickup_borough). */
export function tileFilters(byModel: Record<string, Filter[]>, model: ModelInfo | undefined): Filter[] | undefined {
  if (!model) return undefined
  if (!model.blend) return byModel[model.name]
  return [model.blend.base, ...model.blend.joins.map(j => j.model)]
    .flatMap(m => (byModel[m] ?? []).map(f => ({ ...f, field: `${m}.${f.field}` })))
}

/** The likely same field on another model: the same name, a part of it (pickup_borough:
 *  borough), for a date one of the same kind. '' when nothing fits. */
function suggest(name: string, type: string, options: ModelInfo['dimensions']): string {
  const parts = name.split('_')
  const same = options.find(o => o.name === name)
    ?? options.find(o => parts.includes(o.name) || o.name.split('_').some(p => p.length > 2 && parts.includes(p) && !['at', 'on'].includes(p)))
    ?? (isTemporal(type) ? options.find(o => o.type === type) : undefined)
  return same?.name ?? ''
}

/** For a dashboard field (a filter's, the period's) on `owner`: the other models of the dashboard,
 *  each with the fields it could be on there (dates for a date), and the likely one. */
export function linkChoices(dash: Dashboard, models: ModelInfo[], owner: string, field: string) {
  const own = models.find(m => m.name === owner)?.dimensions.find(d => d.name === field)
  const date = isTemporal(own?.type)
  return dashboardModels(dash, models).filter(m => m.name !== owner).map(m => {
    const options = m.dimensions.filter(d => isTemporal(d.type) === date)
    return { model: m.name, label: m.label ?? m.name, options: options.map(d => choice(m, d)), suggested: suggest(field, own?.type ?? '', options) }
  })
}

/** A filter or period reaching `model` through `field` ('': not reaching it). */
export function withLink<T extends { fields?: Record<string, string> }>(item: T, model: string, field: string): T {
  const { [model]: _old, ...rest } = item.fields ?? {}
  const fields = field ? { ...rest, [model]: field } : rest
  const { fields: _all, ...plain } = item
  return (Object.keys(fields).length ? { ...plain, fields } : plain) as T
}

/** The models a dashboard's tiles read: a blend's own models rather than the blend. */
function dashboardModels(dash: Dashboard, models: ModelInfo[]): ModelInfo[] {
  const names: string[] = []
  for (const t of dash.tiles) {
    const m = t.query && models.find(x => x.name === t.query!.model)
    for (const n of m?.blend ? [m.blend.base, ...m.blend.joins.map(j => j.model)] : m ? [m.name] : []) {
      if (!names.includes(n)) names.push(n)
    }
  }
  return names.map(n => models.find(m => m.name === n)).filter((m): m is ModelInfo => Boolean(m))
}

type Choice = { model: string; field: string; label: string }
const choice = (m: ModelInfo, d: ModelInfo['dimensions'][number]): Choice =>
  ({ model: m.name, field: d.name, label: `${m.label ?? m.name} · ${d.label ?? d.name}` })

/** The fields a dashboard filter may be on: its models' (dates have the period). */
export function filterChoices(dash: Dashboard, models: ModelInfo[]): Choice[] {
  return dashboardModels(dash, models).flatMap(m => m.dimensions.filter(d => !isTemporal(d.type)).map(d => choice(m, d)))
}

/** The dates the dashboard's period may be on (pickup time, or drop-off time, …). */
export function periodChoices(dash: Dashboard, models: ModelInfo[]): Choice[] {
  return dashboardModels(dash, models).flatMap(m => m.dimensions.filter(d => isTemporal(d.type)).map(d => choice(m, d)))
}

export function newFilter(model: string, field: string, op: FilterOp, label: string): DashboardFilter {
  return { model, field, op, default: [], ...(label.trim() ? { label: label.trim() } : {}) }
}
