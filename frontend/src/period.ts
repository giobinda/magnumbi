import { parseIso } from './api'
import type { Compare, Period, PeriodRange, Preset } from './api'

// Compact string form for selects and URL params: "30" | "month_to_date" | "custom"
export const RANGE_OPTIONS: { value: string; label: string }[] = [
  { value: '7', label: 'Last 7 days' }, { value: '14', label: 'Last 14 days' }, { value: '28', label: 'Last 28 days' },
  { value: '30', label: 'Last 30 days' }, { value: '90', label: 'Last 90 days' }, { value: '365', label: 'Last 365 days' },
  { value: 'month_to_date', label: 'Month to date' }, { value: 'quarter_to_date', label: 'Quarter to date' },
  { value: 'year_to_date', label: 'Year to date' }, { value: 'custom', label: 'Custom…' },
]
export const COMPARE_OPTIONS: { value: string; label: string }[] = [
  { value: '', label: 'No comparison' }, { value: 'previous_period', label: 'Previous period' },
  { value: 'previous_year', label: 'Previous year (52 weeks)' }, { value: 'custom', label: 'Custom start…' },
]

export function rangeKey(r: PeriodRange): string {
  if (r.last_n_days) return String(r.last_n_days)
  if (r.preset) return r.preset
  return 'custom'
}

export function rangeFromKey(key: string, prev: PeriodRange): PeriodRange {
  if (/^\d+$/.test(key)) return { last_n_days: Number(key) }
  if (key === 'custom') {
    const today = new Date().toISOString().slice(0, 10)
    return { from: prev.from ?? new Date(Date.now() - 29 * 864e5).toISOString().slice(0, 10), to: prev.to ?? today }
  }
  return { preset: key as Preset }
}

export function compareKey(c: Compare | undefined): string {
  if (!c) return ''
  return typeof c === 'string' ? c : 'custom'
}

export function compareFromKey(key: string, prev: Compare | undefined): Compare {
  if (!key) return null
  if (key === 'custom') return typeof prev === 'object' && prev ? prev : { from: new Date(Date.now() - 395 * 864e5).toISOString().slice(0, 10) }
  return key as Compare
}

/** Apply ?period=30&compare=previous_year URL overrides (used by embeds). */
export function periodFromUrl<P extends Period>(p: P, search: string): P {
  const params = new URLSearchParams(search)
  let out = p
  const r = params.get('period')
  if (r && r !== 'custom') out = { ...out, current: rangeFromKey(r, out.current) }
  const c = params.get('compare')
  if (c !== null) out = { ...out, compare: c === 'none' ? null : compareFromKey(c === 'custom' ? '' : c, out.compare) }
  return out
}

export function fmtDay(iso: string): string {
  const d = parseIso(iso)
  return d.toLocaleDateString(undefined, { day: 'numeric', month: 'short', year: 'numeric', timeZone: 'UTC' })
}

export function windowLabel(w: [string, string]): string {
  return `${fmtDay(w[0])} – ${fmtDay(w[1])}`
}
