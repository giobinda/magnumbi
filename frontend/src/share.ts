/** Public links (anyone with the link sees a dashboard): wording and the no-cache safeguard. */

/** How long a link keeps its results, in words: "1 hour", or "no cache (live)" for 0. */
export function cacheLabel(seconds: number): string {
  if (seconds === 0) return 'no cache (live)'
  const [n, unit] = seconds % 86400 === 0 ? [seconds / 86400, 'day'] : seconds % 3600 === 0 ? [seconds / 3600, 'hour'] : [Math.round(seconds / 60), 'minute']
  return `${n} ${unit}${n === 1 ? '' : 's'}`
}

/** A link without a cache sends every visit to the data warehouse: only once the risks are
 *  ticked as understood and the dashboard's id is typed. */
export function uncachedAccepted(c: { understood: boolean; typed: string }, dashboardId: string): boolean {
  return c.understood && c.typed.trim() === dashboardId
}
