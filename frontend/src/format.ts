import { isTemporal, parseIso } from './api'
import type { Column, Value } from './api'

const nf = new Intl.NumberFormat(undefined, { maximumFractionDigits: 2 })
const cf = new Intl.NumberFormat(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 2 })
const compact = new Intl.NumberFormat(undefined, { notation: 'compact', maximumFractionDigits: 1 })

export function formatNumber(v: number, format?: string | null, short = false): string {
  // small rates keep a decimal even when short: 0.9% vs 1.4% must not both read "1%"
  if (format === 'percent') return `${(v * 100).toFixed(short && Math.abs(v) >= 0.1 ? 0 : 1)}%`
  if (short && Math.abs(v) >= 10_000) return compact.format(v)
  return format === 'currency' && !short ? cf.format(v) : nf.format(v)
}

export function formatChange(v: number): string {
  const pct = v * 100
  if (Math.abs(pct) < 0.05) return '0.0%'
  const digits = Math.abs(pct) >= 100 ? 0 : 1
  return `${pct > 0 ? '+' : ''}${pct.toFixed(digits)}%`
}

export function formatDate(v: string, grain?: string | null): string {
  const d = parseIso(v)
  if (isNaN(d.getTime())) return v
  const y = d.getUTCFullYear(), m = d.getUTCMonth() + 1, day = d.getUTCDate()
  const p = (n: number) => String(n).padStart(2, '0')
  switch (grain) {
    case 'year': return `${y}`
    case 'quarter': return `${y} Q${Math.floor((m - 1) / 3) + 1}`
    case 'month': return `${y}-${p(m)}`
    case 'week': case 'day': return `${y}-${p(m)}-${p(day)}`
    case 'hour': return `${y}-${p(m)}-${p(day)} ${p(d.getUTCHours())}:00`
    default: return v.length > 10 ? `${y}-${p(m)}-${p(day)} ${p(d.getUTCHours())}:${p(d.getUTCMinutes())}` : v
  }
}

export function formatValue(v: Value, col: Column, short = false): string {
  if (v === null || v === undefined) return '∅'
  if (typeof v === 'number') return formatNumber(v, col.format, short)
  if (typeof v === 'boolean') return v ? 'Yes' : 'No'
  if (isTemporal(col.type)) return formatDate(String(v), col.grain)
  return String(v)
}
