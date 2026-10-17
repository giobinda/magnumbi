// More chart types: heatmap, treemap, funnel, waterfall, gauge and histogram (ECharts options),
// and the data behind the pivot table and trend tiles. Pure functions of a query result.
import type { Column, QueryResult, Value } from './api'
import { formatNumber, formatValue } from './format'

const byValue = (a: string, b: string) => a.localeCompare(b, undefined, { numeric: true })

const CALENDARS = [
  ['monday', 'tuesday', 'wednesday', 'thursday', 'friday', 'saturday', 'sunday'],
  ['january', 'february', 'march', 'april', 'may', 'june', 'july', 'august', 'september', 'october', 'november', 'december'],
]

/** Categories in a sensible order: weekdays and months as on a calendar (full or 3-letter names),
 *  everything else by value (numbers as numbers). */
export function orderKeys(keys: string[]): string[] {
  const rank = (k: string, names: string[]) => names.findIndex(n => n === k.toLowerCase() || n.slice(0, 3) === k.toLowerCase())
  const names = CALENDARS.find(c => keys.length > 0 && keys.every(k => rank(k, c) >= 0))
  return names ? [...keys].sort((a, b) => rank(a, names) - rank(b, names)) : [...keys].sort(byValue)
}
const dims = (r: QueryResult) => r.columns.map((c, i) => (c.kind === 'dimension' ? i : -1)).filter(i => i >= 0)
/** the measures themselves (not their previous period or change) */
const measures = (r: QueryResult) => r.columns.map((c, i) => (c.kind === 'measure' && !c.compare ? i : -1)).filter(i => i >= 0)
const num = (v: Value) => (typeof v === 'number' && isFinite(v) ? v : null)

// ------------------------------------------------------------------------------- pivot table

export interface Pivot {
  rowLabel: string; colLabel: string; measure: Column
  rowKeys: string[]; colKeys: string[]; cells: (number | null)[][]
  /** null when the measure doesn't add up (an average): totals would mislead */
  rowTotals: number[] | null; colTotals: number[] | null; total: number | null
}

/** Rows by the first field, columns by the second, the first measure in the cells. */
export function pivot(r: QueryResult): Pivot | null {
  const [d0, d1] = dims(r), m = measures(r)[0]
  if (d0 === undefined || d1 === undefined || m === undefined) return null
  const label = (v: Value, i: number) => formatValue(v, r.columns[i])
  const rowKeys = orderKeys([...new Set(r.rows.map(x => label(x[d0], d0)))])
  const colKeys = orderKeys([...new Set(r.rows.map(x => label(x[d1], d1)))])
  const cells: (number | null)[][] = rowKeys.map(() => colKeys.map(() => null))
  for (const x of r.rows) {
    const [ri, ci] = [rowKeys.indexOf(label(x[d0], d0)), colKeys.indexOf(label(x[d1], d1))]
    const v = num(x[m])
    if (v !== null) cells[ri][ci] = (cells[ri][ci] ?? 0) + v
  }
  const additive = r.columns[m].additive === true
  const sum = (xs: (number | null)[]) => xs.reduce<number>((a, b) => a + (b ?? 0), 0)
  const rowTotals = additive ? cells.map(sum) : null
  const colTotals = additive ? colKeys.map((_, ci) => sum(cells.map(row => row[ci]))) : null
  return {
    rowLabel: r.columns[d0].label, colLabel: r.columns[d1].label, measure: r.columns[m],
    rowKeys, colKeys, cells, rowTotals, colTotals, total: rowTotals ? sum(rowTotals) : null,
  }
}

// ------------------------------------------------------------------------------------ trend

/** The latest value of a measure over time, its change on the value before, and the series
 *  (in time order) for a sparkline. */
export function trendSummary(r: QueryResult): { value: number | null; previous: number | null; change: number | null; points: (number | null)[]; measure: Column } | null {
  const d = dims(r)[0], m = measures(r)[0]
  if (d === undefined || m === undefined) return null
  const rows = [...r.rows].sort((a, b) => byValue(String(a[d] ?? ''), String(b[d] ?? '')))
  const points = rows.map(x => num(x[m]))
  const value = points.at(-1) ?? null, previous = points.length > 1 ? points.at(-2) ?? null : null
  const change = value !== null && previous ? (value - previous) / Math.abs(previous) : null
  return { value, previous, change, points, measure: r.columns[m] }
}

// -------------------------------------------------------------------------------- waterfall

/** Stacked bars for a waterfall: an invisible base under each step, ups and downs, then the
 *  total ('-' = no bar). */
export function waterfallSeries(deltas: number[]) {
  const base: number[] = [], up: (number | '-')[] = [], down: (number | '-')[] = [], total: (number | '-')[] = []
  let running = 0
  for (const d of deltas) {
    if (d >= 0) { base.push(running); up.push(d); down.push('-'); running += d } else { running += d; base.push(running); up.push('-'); down.push(-d) }
    total.push('-')
  }
  base.push(0); up.push('-'); down.push('-'); total.push(running)
  return { base, up, down, total }
}

// -------------------------------------------------------------------------------- histogram

/** `n` equal-width bins from the smallest value to the largest, each value counted `weight` times. */
export function histogramBins(values: number[], weights: number[], n: number) {
  const min = Math.min(...values), max = Math.max(...values)
  const width = (max - min) / n || 1
  const bins = Array.from({ length: n }, (_, i) => ({ from: min + i * width, to: min + (i + 1) * width, count: 0 }))
  values.forEach((v, i) => { bins[Math.min(n - 1, Math.floor((v - min) / width))].count += weights[i] ?? 1 })
  return bins
}

// --------------------------------------------------------------------------- ECharts options

/** A round number at or above `x` (1, 2, 2.5, 5 × a power of ten). */
function niceCeil(x: number): number {
  if (x <= 0) return 1
  const p = 10 ** Math.floor(Math.log10(x))
  return ([1, 2, 2.5, 5, 10].find(s => s * p >= x) ?? 10) * p
}

type Ctx = { base: Record<string, unknown> & { color: string[] }; text: string; grid: string; good: string; bad: string; surface: string }
type Shade = (color: string, towards: string, amount: number) => string
const needs = (ctx: Ctx, what: string) => ({ ...ctx.base, title: { text: what, textStyle: { fontSize: 12, color: ctx.text } } })

/** The option for one of the types above, or null when `type` isn't one of them. */
export function moreOption(type: string, r: QueryResult, opts: Record<string, unknown>, ctx: Ctx, shade: Shade): Record<string, unknown> | null {
  const cols = r.columns, [d0, d1] = dims(r), m = measures(r)[0]
  const fmt = (v: number) => formatNumber(v, m === undefined ? null : cols[m].format)
  const valueAxis = { type: 'value', axisLabel: { color: ctx.text, formatter: (v: number) => formatNumber(v, cols[m]?.format, true) }, splitLine: { lineStyle: { color: ctx.grid } } }
  const catAxis = (data: string[]) => ({ type: 'category', data, axisLabel: { color: ctx.text, hideOverlap: true }, axisLine: { lineStyle: { color: ctx.grid } }, axisTick: { show: false } })

  switch (type) {
    case 'heatmap': {
      if (d0 === undefined || d1 === undefined || m === undefined) return needs(ctx, 'Heatmap needs 2 dimensions + 1 measure')
      const xs = orderKeys([...new Set(r.rows.map(x => formatValue(x[d0], cols[d0])))])
      const ys = orderKeys([...new Set(r.rows.map(x => formatValue(x[d1], cols[d1])))])
      const data = r.rows.filter(x => num(x[m]) !== null).map(x => [xs.indexOf(formatValue(x[d0], cols[d0])), ys.indexOf(formatValue(x[d1], cols[d1])), x[m] as number])
      const values = data.map(x => x[2])
      return {
        ...ctx.base,
        tooltip: { trigger: 'item', formatter: (p: { data: number[] }) => `${xs[p.data[0]]} · ${ys[p.data[1]]}: <b>${fmt(p.data[2])}</b>` },
        grid: { left: 8, right: 16, top: 12, bottom: 48, containLabel: true },
        // the first row at the top, as in a table
        xAxis: { ...catAxis(xs), splitArea: { show: false } }, yAxis: { ...catAxis(ys), inverse: true, splitArea: { show: false } },
        visualMap: { min: Math.min(...values), max: Math.max(...values), calculable: false, orient: 'horizontal', left: 'center', bottom: 0,
          itemHeight: 120, textStyle: { color: ctx.text }, formatter: (v: number) => formatNumber(v, cols[m].format, true),
          inRange: { color: [shade(ctx.base.color[0], ctx.surface, 0.9), ctx.base.color[0]] } },
        series: [{ type: 'heatmap', data, itemStyle: { borderColor: ctx.surface, borderWidth: 1 }, emphasis: { itemStyle: { borderColor: ctx.text } } }],
      }
    }
    case 'funnel': {
      if (d0 === undefined || m === undefined) return needs(ctx, 'Funnel needs 1 dimension + 1 measure')
      return {
        ...ctx.base, tooltip: { trigger: 'item', formatter: (p: { name: string; value: number }) => `${p.name}: <b>${fmt(p.value)}</b>` },
        series: [{ type: 'funnel', sort: 'none', left: '10%', width: '80%', top: 8, bottom: 8, gap: 2,
          label: { position: 'inside', color: '#fff', formatter: (p: { name: string; value: number }) => `${p.name}\n${formatNumber(p.value, cols[m].format, true)}` },
          itemStyle: { borderColor: ctx.surface, borderWidth: 1 },
          data: r.rows.map(x => ({ name: formatValue(x[d0], cols[d0]), value: x[m] })) }],
      }
    }
    case 'treemap': {
      if (d0 === undefined || m === undefined) return needs(ctx, 'Treemap needs 1 or 2 dimensions + 1 measure')
      const groups = new Map<string, { name: string; value: number; children: { name: string; value: number }[] }>()
      for (const x of r.rows) {
        const name = formatValue(x[d0], cols[d0]), v = num(x[m]) ?? 0
        const g = groups.get(name) ?? { name, value: 0, children: [] }
        g.value += v
        if (d1 !== undefined) g.children.push({ name: formatValue(x[d1], cols[d1]), value: v })
        groups.set(name, g)
      }
      const data = [...groups.values()].sort((a, b) => byValue(a.name, b.name)).map(g => (d1 === undefined ? { name: g.name, value: g.value } : g))
      return {
        ...ctx.base, tooltip: { trigger: 'item', formatter: (p: { name: string; value: number }) => `${p.name}: <b>${fmt(p.value)}</b>` },
        series: [{ type: 'treemap', roam: false, nodeClick: false, breadcrumb: { show: false }, top: 4, left: 4, right: 4, bottom: 4,
          label: { show: true, formatter: '{b}' }, upperLabel: { show: d1 !== undefined, height: 20, color: '#fff' },
          levels: [{ itemStyle: { borderColor: ctx.surface, borderWidth: 2, gapWidth: 2 } }, { colorSaturation: [0.35, 0.6], itemStyle: { gapWidth: 1, borderColorSaturation: 0.6 } }],
          data }],
      }
    }
    case 'gauge': {
      if (m === undefined) return needs(ctx, 'Gauge needs 1 measure')
      const value = num(r.rows[0]?.[m]) ?? 0
      const target = typeof opts.target === 'number' && opts.target > 0 ? opts.target : null
      return {
        ...ctx.base,
        series: [{ type: 'gauge', min: 0, max: target ?? niceCeil(value * 1.25), startAngle: 200, endAngle: -20, radius: '95%', center: ['50%', '60%'],
          progress: { show: true, width: 14, itemStyle: { color: ctx.base.color[0] } }, axisLine: { lineStyle: { width: 14, color: [[1, ctx.grid]] } },
          pointer: { show: false }, axisTick: { show: false }, splitLine: { show: false },
          axisLabel: { show: false },
          title: { show: true, offsetCenter: [0, '30%'], color: ctx.text, fontSize: 12 },
          detail: { valueAnimation: true, offsetCenter: [0, '-5%'], fontSize: 22, fontWeight: 700, color: 'inherit', formatter: (v: number) => formatNumber(v, cols[m].format, true) },
          data: [{ value, name: target ? `${Math.round((value / target) * 100)}% of ${formatNumber(target, cols[m].format, true)}` : `of ${formatNumber(niceCeil(value * 1.25), cols[m].format, true)}` }] }],
      }
    }
    case 'waterfall': {
      if (d0 === undefined || m === undefined) return needs(ctx, 'Waterfall needs 1 dimension + 1 measure')
      const labels = [...r.rows.map(x => formatValue(x[d0], cols[d0])), String(opts.total_label ?? 'Total')]
      const w = waterfallSeries(r.rows.map(x => num(x[m]) ?? 0))
      const bar = (name: string, data: unknown[], color?: string) => ({ name, type: 'bar', stack: 'w', data, barMaxWidth: 36, itemStyle: color ? { color, borderRadius: 2 } : undefined })
      return {
        ...ctx.base, grid: { left: 8, right: 16, top: 12, bottom: 8, containLabel: true },
        tooltip: { trigger: 'axis', axisPointer: { type: 'shadow' }, formatter: (ps: { seriesName: string; value: unknown; axisValueLabel: string }[]) => {
          const p = ps.find(x => x.seriesName !== 'base' && typeof x.value === 'number')
          return p ? `${p.axisValueLabel}: <b>${p.seriesName === 'down' ? '−' : ''}${fmt(p.value as number)}</b>` : ''
        } },
        xAxis: catAxis(labels), yAxis: valueAxis,
        series: [{ ...bar('base', w.base), itemStyle: { color: 'transparent' }, emphasis: { disabled: true } },
          bar('up', w.up, ctx.good), bar('down', w.down, ctx.bad), bar('total', w.total, ctx.base.color[0])],
      }
    }
    case 'histogram': {
      if (d0 === undefined) return needs(ctx, 'Histogram needs a number dimension (and a count)')
      const pts = r.rows.filter(x => num(x[d0]) !== null)
      if (!pts.length) return needs(ctx, 'Histogram needs a number dimension (and a count)')
      const n = typeof opts.bins === 'number' ? opts.bins : Math.min(30, Math.max(5, Math.ceil(Math.sqrt(pts.length))))
      const bins = histogramBins(pts.map(x => x[d0] as number), pts.map(x => (m === undefined ? 1 : num(x[m]) ?? 0)), n)
      const label = (b: { from: number; to: number }) => `${formatNumber(b.from, null, true)}–${formatNumber(b.to, null, true)}`
      return {
        ...ctx.base, grid: { left: 8, right: 16, top: 12, bottom: 8, containLabel: true },
        tooltip: { trigger: 'axis', axisPointer: { type: 'shadow' } },
        xAxis: catAxis(bins.map(label)), yAxis: { ...valueAxis, axisLabel: { color: ctx.text } },
        series: [{ type: 'bar', name: m === undefined ? 'rows' : cols[m].label, data: bins.map(b => b.count), barCategoryGap: '4%', itemStyle: { borderRadius: [2, 2, 0, 0] } }],
      }
    }
  }
  return null
}
