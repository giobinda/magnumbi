/** Guest links (a dashboard for whoever has the link and its password): wording and routing. */

/** How long a link can last, in hours: an hour to 14 days (the server's limit). */
export const EXPIRY_HOURS = [1, 24, 72, 168, 336]

export function hoursLabel(hours: number): string {
  const [n, unit] = hours % 24 === 0 ? [hours / 24, 'day'] : [hours, 'hour']
  return `${n} ${unit}${n === 1 ? '' : 's'}`
}

type Rule = { model: string; dimension: string; values: unknown[] }
type ModelLike = { name: string; dimensions: { name: string; label?: string | null }[] }

/** What a link's guests are limited to: "Region: EMEA, APAC; Store: Store 01". */
export function restrictionsLabel(rules: Rule[], models: ModelLike[]): string {
  return rules.map(r => {
    const d = models.find(m => m.name === r.model)?.dimensions.find(x => x.name === r.dimension)
    return `${d?.label ?? r.dimension}: ${r.values.map(String).join(', ')}`
  }).join('; ')
}

/** Where a dashboard's requests go: to the guest link while one is open, else the dashboard's
 *  own endpoints. */
export function guestPath(base: string | null, dashboard: string,
  what: { tile: string } | { key: string } | { model: string } | { export: string | null }): string {
  const e = encodeURIComponent
  if ('export' in what) {  // a tile's CSV, or (null) the dashboard's zip
    const tile = what.export === null ? '' : `/tiles/${e(what.export)}`
    return `${base ?? `/api/dashboards/${e(dashboard)}`}${tile}/export`
  }
  if ('tile' in what) return base ? `${base}/tiles/${e(what.tile)}/query` : `/api/dashboards/${e(dashboard)}/tiles/${e(what.tile)}/query`
  if ('key' in what) return `${base ?? `/api/dashboards/${e(dashboard)}`}/filters/values?key=${e(what.key)}`
  return `${base ?? '/api'}/period?model=${e(what.model)}`
}

type WithBlend = ModelLike & { blend?: { base: string; joins: { model: string }[] } | null }

/** The models a guest link's limits can be on, for a dashboard using `used`: a blend's own
 *  models in its place (a blend has no restrictions of its own). In the list's order. */
export function limitModels<M extends WithBlend>(used: string[], models: M[]): M[] {
  const names = new Set(used.flatMap(n => {
    const b = models.find(m => m.name === n)?.blend
    return b ? [b.base, ...b.joins.map(j => j.model)] : [n]
  }))
  return models.filter(m => names.has(m.name))
}
