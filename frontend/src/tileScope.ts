// Which of the dashboard's filters (and period) a tile follows. Its author may have it ignore
// some (`options.ignore_filters`, `options.period: false`): a chart that keeps its own context. Such
// a tile says so, as does one with filters of its own, so readers know it differs from the rest.
import { fieldLabel } from './api'
import type { ChartTile, Dashboard, ModelInfo } from './api'

export type Input = { key: string; label: string }

/** The dashboard's filters and period that reach this tile: on its model, or a blend with it. */
export function dashboardInputs(dash: Dashboard, tile: ChartTile, model?: ModelInfo): Input[] {
  const ref = (m: string, field: string) =>
    tile.query.model === m ? field : model?.dimensions.some(d => d.name === `${m}.${field}`) ? `${m}.${field}` : null
  const out: Input[] = []
  for (const f of dash.filters) {
    const r = ref(f.model, f.field)
    if (r) out.push({ key: `${f.model}.${f.field}`, label: f.label || fieldLabel(model, r) })
  }
  const p = dash.period
  if (p && !tile.query.period && ref(p.model, p.field)) out.push({ key: 'period', label: 'Period' })
  return out
}

/** What a tile ignores: dashboard filters by `model.field`, and `period`. */
export function ignoredOf(options?: Record<string, unknown>): string[] {
  const filters = Array.isArray(options?.ignore_filters) ? options.ignore_filters.filter(k => typeof k === 'string') : []
  return [...(options?.period === false ? ['period'] : []), ...filters]
}

/** The tile's options ignoring `ignored`, its other options as they are. */
export function withIgnored(options: Record<string, unknown>, ignored: string[]): Record<string, unknown> {
  const { period: _period, ignore_filters: _filters, ...rest } = options
  const filters = ignored.filter(k => k !== 'period')
  return {
    ...rest,
    ...(ignored.includes('period') ? { period: false } : {}),
    ...(filters.length ? { ignore_filters: filters } : {}),
  }
}

const and = (xs: string[]) => (xs.length > 1 ? `${xs.slice(0, -1).join(', ')} and ${xs[xs.length - 1]}` : xs[0])

/** The tile's badge when it's filtered differently from the rest of the dashboard (else null). */
export function scopeNote(dash: Dashboard, tile: ChartTile, model?: ModelInfo): { short: string; detail: string } | null {
  if (!dash.filters.length && !dash.period) return null
  const ignored = ignoredOf(tile.options)
  const ignores = dashboardInputs(dash, tile, model).filter(i => ignored.includes(i.key)).map(i => i.label)
  const own = (tile.query.filters ?? []).map(f => fieldLabel(model, f.field))
  const ownPeriod = Boolean(tile.query.period)
  if (!ignores.length && !own.length && !ownPeriod) return null
  const detail = [
    ignores.length ? `Doesn't follow the dashboard's ${and(ignores)}.` : '',
    own.length ? `Its own filters: ${own.join(', ')}.` : '',
    ownPeriod ? 'Its own period.' : '',
  ].filter(Boolean).join(' ')
  const short = !ignores.length ? 'Own filters'
    : ignores.length > 2 ? `Ignores ${ignores.length} dashboard filters` : `Ignores ${ignores.join(', ')}`
  return { short, detail }
}
