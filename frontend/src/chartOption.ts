// ECharts options for bar / line / area / pie / scatter tiles: a pure function of the result,
// so it can be tested without a browser.
import type { ChartType, Column, QueryResult, Value } from './api'
import { formatChange, formatNumber, formatValue } from './format'

/** Tooltips are HTML: every value from the data (or config) is escaped before it goes in. */
export const esc = (v: unknown) => String(v ?? '').replace(/[&<>"']/g, c => `&#${c.charCodeAt(0)};`)

/** `color` moved `amount` (0..1) of the way towards `towards` (hex colours; others unchanged). */
export function shade(color: string, towards: string, amount: number): string {
  const rgb = (c: string) => {
    const h = /^#([0-9a-f]{3}|[0-9a-f]{6})$/i.exec(c.trim())?.[1]
    if (!h) return null
    const full = h.length === 3 ? [...h].map(x => x + x).join('') : h
    return [0, 2, 4].map(i => parseInt(full.slice(i, i + 2), 16))
  }
  const [a, b] = [rgb(color), rgb(towards)]
  if (!a || !b) return color
  return '#' + a.map((x, i) => Math.round(x + (b[i] - x) * amount).toString(16).padStart(2, '0')).join('')
}

export type Option = Record<string, unknown> & { __drill?: { column: Column; raw: Value[] } }

function cssVar(name: string, fallback: string) {
  if (typeof document === 'undefined') return fallback
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim() || fallback
}

/** Values in order, numbers as numbers ('9' before '10'): the order colours are given in. */
const byValue = (a: string, b: string) => a.localeCompare(b, undefined, { numeric: true })

export function buildOption(result: QueryResult, type: ChartType, opts: Record<string, unknown>, palette: string[]): Option {
  const text = cssVar('--text-muted', '#667085'), grid = cssVar('--border', '#e4e7ec')
  const cols = result.columns
  const dimIdx = cols.map((c, i) => (c.kind === 'dimension' ? i : -1)).filter(i => i >= 0)
  // Δ% columns are shown in tables/KPIs and tooltips, not plotted
  const meaIdx = cols.map((c, i) => (c.kind === 'measure' && c.compare !== 'change' ? i : -1)).filter(i => i >= 0)
  const comparing = cols.some(c => c.compare === 'previous')
  const base = {
    // series beyond the palette are neutral grey: colours are never cycled (two series, one colour)
    color: [...palette, ...Array(Math.max(0, 40 - palette.length)).fill(cssVar('--text-muted', '#888888'))],
    textStyle: { fontFamily: 'inherit' },
    tooltip: { trigger: type === 'pie' || type === 'scatter' ? 'item' : 'axis', confine: true },
    animationDuration: 300,
  }

  if (type === 'pie') {
    const d = dimIdx[0], m = meaIdx[0]
    if (d === undefined || m === undefined) return { ...base, title: { text: 'Pie needs 1 dimension + 1 measure', textStyle: { fontSize: 12, color: text } } }
    const names = result.rows.map(r => formatValue(r[d], cols[d]))
    const ordered = [...names].sort(byValue)
    return {
      ...base,
      tooltip: { trigger: 'item', valueFormatter: (v: number) => formatNumber(v, cols[m].format) },
      legend: { type: 'scroll', bottom: 0, textStyle: { color: text } },
      series: [{
        type: 'pie', radius: ['40%', '70%'], center: ['50%', '45%'], itemStyle: { borderColor: cssVar('--surface', '#fff'), borderWidth: 2 },
        label: { show: false }, emphasis: { label: { show: true, formatter: '{b}\n{d}%', fontWeight: 600, color: text } },
        // colours by value, like split series: a value has one colour across a dashboard's charts
        data: result.rows.map((r, i) => ({ name: names[i], value: r[m], itemStyle: { color: base.color[ordered.indexOf(names[i])] } })),
      }],
      __drill: { column: cols[d], raw: result.rows.map(r => r[d]) },
    }
  }

  if (type === 'scatter') {
    const [mx, my] = meaIdx, d = dimIdx[0]
    if (my === undefined) return { ...base, title: { text: 'Scatter needs 2 measures', textStyle: { fontSize: 12, color: text } } }
    return {
      ...base,
      grid: { left: 12, right: 24, top: 24, bottom: 28, containLabel: true },
      xAxis: { name: cols[mx].label, nameLocation: 'middle', nameGap: 28, axisLabel: { color: text, formatter: (v: number) => formatNumber(v, cols[mx].format, true) }, splitLine: { lineStyle: { color: grid } } },
      yAxis: { name: cols[my].label, axisLabel: { color: text, formatter: (v: number) => formatNumber(v, cols[my].format, true) }, splitLine: { lineStyle: { color: grid } } },
      tooltip: { trigger: 'item', formatter: (p: { data: Value[] }) => `${d !== undefined ? esc(formatValue(p.data[2], cols[d])) + '<br/>' : ''}${esc(cols[mx].label)}: ${esc(formatNumber(p.data[0] as number, cols[mx].format))}<br/>${esc(cols[my].label)}: ${esc(formatNumber(p.data[1] as number, cols[my].format))}` },
      series: [{ type: 'scatter', symbolSize: 10, data: result.rows.map(r => [r[mx], r[my], d !== undefined ? r[d] : null]) }],
      ...(d !== undefined ? { __drill: { column: cols[d], raw: result.rows.map(r => r[d]) } } : {}),
    }
  }

  // bar / hbar / line / area
  const d0 = dimIdx[0], d1 = dimIdx[1]
  const categoriesRaw: Value[] = []
  const catKey = new Map<string, number>()
  const seriesMap = new Map<string, { name: string; measure: Column; previous: boolean; data: (number | null)[] }>()

  for (const r of result.rows) {
    const raw = d0 !== undefined ? r[d0] : 'Total'
    const key = String(raw)
    if (!catKey.has(key)) { catKey.set(key, categoriesRaw.length); categoriesRaw.push(raw) }
  }
  // 2 dims + 1 measure -> one series per 2nd-dim value. A period comparison isn't plotted then:
  // a previous series per value would be too many (it stays in tables and KPIs)
  const current = meaIdx.filter(m => cols[m].compare !== 'previous')
  const pivot = d1 !== undefined && current.length === 1
  for (const r of result.rows) {
    const ci = catKey.get(String(d0 !== undefined ? r[d0] : 'Total'))!
    for (const m of pivot ? current : meaIdx) {
      const sKey = pivot ? String(r[d1!]) : cols[m].name
      const sName = pivot ? formatValue(r[d1!], cols[d1!]) : cols[m].label
      if (!seriesMap.has(sKey)) seriesMap.set(sKey, { name: sName, measure: cols[m], previous: cols[m].compare === 'previous', data: Array(categoriesRaw.length).fill(null) })
      seriesMap.get(sKey)!.data[ci] = r[m] as number
    }
  }
  // split series in value order (numbers as numbers), so a value keeps its colour from chart to chart
  const series = [...seriesMap.values()]
  if (pivot) series.sort((a, b) => byValue(a.name, b.name))
  // a previous-period series: its measure's colour, half-way to the background (lighter on light
  // themes, darker on dark ones), and dashed on lines
  const colorOf = new Map<string, string>()
  series.filter(s => !s.previous).forEach((s, i) => colorOf.set(s.measure.name, palette[i] ?? cssVar('--text-muted', '#888888')))
  const background = cssVar('--surface', '#ffffff')
  const seriesColor = (s: (typeof series)[number]) => {
    if (pivot) return undefined
    const c = colorOf.get(s.previous ? s.measure.base! : s.measure.name)
    return c && s.previous ? shade(c, background, 0.5) : c
  }
  const changeOf = (name: string, ci: number) => {
    const idx = cols.findIndex(c => c.base === name && c.compare === 'change')
    if (idx < 0) return undefined
    const row = result.rows.find(r => catKey.get(String(d0 !== undefined ? r[d0] : 'Total')) === ci)
    return row?.[idx]
  }
  const stacked = Boolean(opts.stacked) || (pivot && type === 'area')
  const horizontal = type === 'hbar'
  // a second scale for two measures (trips vs temperature: on one scale the smaller looks flat),
  // or for measures in different formats (revenue vs orders); a previous-period series goes on
  // its measure's scale. Not for stacks (one total) or horizontal bars.
  const measureOf = (s: (typeof series)[number]) => (s.previous ? s.measure.base! : s.measure.name)
  const measures = [...new Set(series.map(measureOf))]
  const formats = [...new Set(series.map(s => s.measure.format ?? 'number'))]
  const dual = !pivot && !stacked && !horizontal && (measures.length === 2 || formats.length > 1)
  const axisFor = (s: (typeof series)[number]) => {
    if (!dual) return 0
    if (measures.length === 2) return measures.indexOf(measureOf(s))
    return (s.measure.format ?? 'number') !== formats[0] ? 1 : 0
  }
  const catLabels = categoriesRaw.map(v => (d0 !== undefined ? formatValue(v, cols[d0]) : String(v)))

  const valueAxis = (i: number) => {
    const first = series.find(s => axisFor(s) === i && !s.previous)
    const fmt = first?.measure.format
    // two measures: each scale says whose it is, in its series' colour
    const named = dual && measures.length === 2 && first
    return {
      type: 'value', position: i ? 'right' : undefined,
      name: named ? first.name : undefined,
      nameTextStyle: named ? { color: seriesColor(first) ?? text, align: i ? 'right' : 'left', fontWeight: 600 } : undefined,
      axisLabel: { color: text, formatter: (v: number) => formatNumber(v, fmt, true) },
      splitLine: { show: i === 0, lineStyle: { color: grid } },
    }
  }
  const catAxis = {
    type: 'category', data: horizontal ? [...catLabels].reverse() : catLabels,
    axisLabel: { color: text, hideOverlap: true }, axisLine: { lineStyle: { color: grid } }, axisTick: { show: false },
  }
  const n = categoriesRaw.length
  return {
    ...base,
    legend: series.length > 1 ? { type: 'scroll', top: 0, textStyle: { color: text } } : undefined,
    grid: { left: 8, right: 16, top: dual && measures.length === 2 ? 52 : series.length > 1 ? 32 : 12, bottom: 8, containLabel: true },
    tooltip: {
      trigger: 'axis', confine: true, axisPointer: { type: type === 'line' || type === 'area' ? 'line' : 'shadow' },
      valueFormatter: undefined,
      formatter: (ps: { seriesIndex: number; dataIndex: number; marker: string; seriesName: string; value: number; axisValueLabel: string }[]) =>
        `${esc(ps[0]?.axisValueLabel ?? '')}<br/>` + ps.map(p => {
          const s = series[p.seriesIndex]
          const v = `${p.marker}${esc(p.seriesName)}: <b>${p.value == null ? '–' : esc(formatNumber(p.value, s.measure.format))}</b>`
          const ch = !s.previous && comparing && !pivot ? changeOf(s.measure.name, horizontal ? n - 1 - p.dataIndex : p.dataIndex) : undefined
          return typeof ch === 'number' ? `${v} <span style="opacity:.75">(${esc(formatChange(ch))})</span>` : v
        }).join('<br/>'),
    },
    xAxis: horizontal ? valueAxis(0) : catAxis,
    yAxis: horizontal ? catAxis : dual ? [valueAxis(0), valueAxis(1)] : valueAxis(0),
    series: series.map(s => ({
      name: s.name,
      type: type === 'area' ? 'line' : horizontal ? 'bar' : type,
      data: horizontal ? [...s.data].reverse() : s.data,
      yAxisIndex: horizontal ? undefined : axisFor(s),
      stack: stacked ? (s.previous ? 'previous' : 'total') : undefined,
      color: seriesColor(s),
      lineStyle: s.previous ? { type: 'dashed', width: 1.5 } : undefined,
      areaStyle: type === 'area' ? { opacity: s.previous ? 0.08 : 0.25 } : undefined,
      z: s.previous ? 1 : 2,
      smooth: type === 'line' || type === 'area' ? 0.2 : undefined,
      showSymbol: n < 40,
      barMaxWidth: 36,
      itemStyle: type === 'bar' || horizontal ? { borderRadius: horizontal ? [0, 3, 3, 0] : [3, 3, 0, 0] } : undefined,
    })),
    __drill: d0 !== undefined ? { column: cols[d0], raw: horizontal ? [...categoriesRaw].reverse() : categoriesRaw } : undefined,
  }
}
