import type { Value } from './api'

/** A KPI's line against the comparison period: its change and earlier value, or 'none' when the
 *  comparison period had nothing (no rows, or zero): "– vs 0" would look broken. */
export function comparison(change: Value, previous: Value): 'none' | { change: Value; previous: Value } {
  const hasChange = typeof change === 'number' && isFinite(change)
  const nothing = previous === null || previous === undefined || previous === 0
  return !hasChange && nothing ? 'none' : { change, previous }
}
