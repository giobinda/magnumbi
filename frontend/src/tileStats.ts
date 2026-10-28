import type { QueryResult } from './api'

/** How a tile's data came: rows, time, the cache. Under the tile while editing, in its title's
 *  tooltip otherwise (a dashboard stays quiet). */
export function tileStats(data: QueryResult): string {
  const parts = [`${data.rows.length} rows`]
  if (data.stats.duration_ms != null) parts.push(`${data.stats.duration_ms} ms`)
  if (data.stats.cache_hit) parts.push('cached')
  return parts.join(' · ')
}
