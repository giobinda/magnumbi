import { useMemo, useState } from 'react'
import type { ChartType, Column, QueryResult, Value } from '../api'
import { formatChange, formatNumber, formatValue } from '../format'
import EChart from './EChart'
import { useTheme } from '../theme'

/** Tooltips are HTML: every value from the data (or config) is escaped before it goes in. */
const esc = (v: unknown) => String(v ?? '').replace(/[&<>"']/g, c => `&#${c.charCodeAt(0)};`)


export interface DrillClick { column: Column; value: Value }

type Props = {
  result: QueryResult
  type: ChartType
  options?: Record<string, unknown>
  onDrill?: (c: DrillClick) => void
}

export default function Chart({ result, type, options, onDrill }: Props) {
  if (!result.rows.length) return <div className="empty">No data</div>
  if (type === 'kpi') return <Kpi result={result} />
  if (type === 'table') return <Table result={result} onDrill={onDrill} />
  return <EChartView result={result} type={type} options={options} onDrill={onDrill} />
}

/** ▲ +12.3% coloured by whether the move is good for this measure. */
export function Change({ value, higherIsBetter }: { value: Value; higherIsBetter?: boolean | null }) {
  if (typeof value !== 'number' || !isFinite(value)) return <span className="change flat">–</span>
  const good = Math.abs(value) < 0.0005 ? null : (value > 0) === (higherIsBetter !== false)
  return (
    <span className={`change ${good === null ? 'flat' : good ? 'good' : 'bad'}`}>
      {good === null ? '■' : value > 0 ? '▲' : '▼'} {formatChange(value)}
    </span>
  )
}

function Kpi({ result }: { result: QueryResult }) {
  const cols = result.columns
  const measures = cols.map((c, i) => [c, i] as const).filter(([c]) => c.kind === 'measure' && !c.compare)
  const row = result.rows[0]
  const find = (base: string, kind: string) => cols.findIndex(c => c.base === base && c.compare === kind)
  return (
    <div className="kpi">
      {measures.map(([c, i]) => {
        const pi = find(c.name, 'previous'), ci = find(c.name, 'change')
        return (
          <div key={c.name} className="kpi-item">
            <div className="kpi-value">{typeof row[i] === 'number' ? formatNumber(row[i] as number, c.format, true) : formatValue(row[i], c)}</div>
            {measures.length > 1 && <div className="kpi-label">{c.label}</div>}
            {ci >= 0 && (
              <div className="kpi-compare">
                <Change value={row[ci]} higherIsBetter={c.higher_is_better} />
                <span className="muted"> vs {typeof row[pi] === 'number' ? formatNumber(row[pi] as number, c.format, true) : '–'}</span>
              </div>
            )}
          </div>
        )
      })}
    </div>
  )
}

function Table({ result, onDrill }: { result: QueryResult; onDrill?: (c: DrillClick) => void }) {
  const [sort, setSort] = useState<{ i: number; desc: boolean } | null>(null)
  const rows = useMemo(() => {
    if (!sort) return result.rows
    const { i, desc } = sort
    return [...result.rows].sort((a, b) => {
      const x = a[i], y = b[i]
      const cmp = x === null ? -1 : y === null ? 1 : x < y ? -1 : x > y ? 1 : 0
      return desc ? -cmp : cmp
    })
  }, [result.rows, sort])
  const firstDim = result.columns.findIndex(c => c.kind === 'dimension')
  return (
    <div className="table-wrap">
      <table className="data">
        <thead>
          <tr>{result.columns.map((c, i) => (
            <th key={c.name} className={(c.kind === 'measure' ? 'num' : '') + (c.compare === 'previous' ? ' prev' : '')}
              onClick={() => setSort(s => ({ i, desc: s?.i === i ? !s.desc : c.kind === 'measure' }))}>
              {c.label}{sort?.i === i ? (sort.desc ? ' ↓' : ' ↑') : ''}
            </th>
          ))}</tr>
        </thead>
        <tbody>
          {rows.map((r, ri) => (
            <tr key={ri}>{r.map((v, i) => {
              const c = result.columns[i]
              const drillable = onDrill && i === firstDim
              return (
                <td key={i} className={(c.kind === 'measure' ? 'num' : '') + (c.compare === 'previous' ? ' prev' : '') + (drillable ? ' drillable' : '')}
                  onClick={drillable ? () => onDrill!({ column: c, value: v }) : undefined}>
                  {c.compare === 'change' ? <Change value={v} higherIsBetter={c.higher_is_better} /> : formatValue(v, c)}
                </td>
              )
            })}</tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}

function EChartView({ result, type, options, onDrill }: Props) {
  const { theme } = useTheme()
  const { drill, echartsOption } = useMemo(() => {
    const { __drill, ...rest } = buildOption(result, type, options ?? {}, theme.palette)
    return { drill: __drill, echartsOption: rest }
  }, [result, type, options, theme])
  const handleClick = onDrill && drill
    ? (p: { dataIndex: number }) => {
        const value = drill.raw[p.dataIndex]
        if (value !== undefined) onDrill({ column: drill.column, value })
      }
    : undefined
  return <EChart option={echartsOption} onClick={handleClick as never} />
}

type Option = Record<string, unknown> & { __drill?: { column: Column; raw: Value[] } }

function cssVar(name: string, fallback: string) {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim() || fallback
}

function buildOption(result: QueryResult, type: ChartType, opts: Record<string, unknown>, palette: string[]): Option {
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
    return {
      ...base,
      tooltip: { trigger: 'item', valueFormatter: (v: number) => formatNumber(v, cols[m].format) },
      legend: { type: 'scroll', bottom: 0, textStyle: { color: text } },
      series: [{
        type: 'pie', radius: ['40%', '70%'], center: ['50%', '45%'], itemStyle: { borderColor: cssVar('--surface', '#fff'), borderWidth: 2 },
        label: { show: false }, emphasis: { label: { show: true, formatter: '{b}\n{d}%', fontWeight: 600, color: text } },
        data: result.rows.map(r => ({ name: formatValue(r[d], cols[d]), value: r[m] })),
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
  // 2 dims + 1 measure -> one series per 2nd-dim value (not combined with period comparison)
  const pivot = d1 !== undefined && meaIdx.length === 1 && !comparing
  for (const r of result.rows) {
    const ci = catKey.get(String(d0 !== undefined ? r[d0] : 'Total'))!
    for (const m of meaIdx) {
      const sKey = pivot ? String(r[d1!]) : cols[m].name
      const sName = pivot ? formatValue(r[d1!], cols[d1!]) : cols[m].label
      if (!seriesMap.has(sKey)) seriesMap.set(sKey, { name: sName, measure: cols[m], previous: cols[m].compare === 'previous', data: Array(categoriesRaw.length).fill(null) })
      seriesMap.get(sKey)!.data[ci] = r[m] as number
    }
  }
  const series = [...seriesMap.values()]
  // a previous-period series reuses its measure's colour, faded (bars) or dashed (lines)
  const colorOf = new Map<string, string>()
  series.filter(s => !s.previous).forEach((s, i) => colorOf.set(s.measure.name, palette[i] ?? cssVar('--text-muted', '#888888')))
  const seriesColor = (s: (typeof series)[number]) => (pivot ? undefined : colorOf.get(s.previous ? s.measure.base! : s.measure.name))
  const changeOf = (name: string, ci: number) => {
    const idx = cols.findIndex(c => c.base === name && c.compare === 'change')
    if (idx < 0) return undefined
    const row = result.rows.find(r => catKey.get(String(d0 !== undefined ? r[d0] : 'Total')) === ci)
    return row?.[idx]
  }
  // second axis when measures have different formats (e.g. revenue vs orders)
  const formats = [...new Set(series.map(s => s.measure.format ?? 'number'))]
  const dual = !pivot && formats.length > 1 && type !== 'hbar'
  const axisFor = (s: (typeof series)[number]) => (dual && (s.measure.format ?? 'number') !== formats[0] ? 1 : 0)
  const stacked = Boolean(opts.stacked) || (pivot && type === 'area')
  const horizontal = type === 'hbar'
  const catLabels = categoriesRaw.map(v => (d0 !== undefined ? formatValue(v, cols[d0]) : String(v)))

  const valueAxis = (i: number) => {
    const fmt = series.find(s => axisFor(s) === i)?.measure.format
    return {
      type: 'value', position: i ? 'right' : undefined,
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
    grid: { left: 8, right: 16, top: series.length > 1 ? 32 : 12, bottom: 8, containLabel: true },
    tooltip: {
      trigger: 'axis', confine: true, axisPointer: { type: type === 'line' || type === 'area' ? 'line' : 'shadow' },
      valueFormatter: undefined,
      formatter: (ps: { seriesIndex: number; dataIndex: number; marker: string; seriesName: string; value: number; axisValueLabel: string }[]) =>
        `${esc(ps[0]?.axisValueLabel ?? '')}<br/>` + ps.map(p => {
          const s = series[p.seriesIndex]
          const v = `${p.marker}${esc(p.seriesName)}: <b>${p.value == null ? '–' : esc(formatNumber(p.value, s.measure.format))}</b>`
          const ch = !s.previous && comparing ? changeOf(s.measure.name, horizontal ? n - 1 - p.dataIndex : p.dataIndex) : undefined
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
      itemStyle: type === 'bar' || horizontal
        ? { borderRadius: horizontal ? [0, 3, 3, 0] : [3, 3, 0, 0], opacity: s.previous ? 0.4 : 1 }
        : s.previous ? { opacity: 0.6 } : undefined,
    })),
    __drill: d0 !== undefined ? { column: cols[d0], raw: horizontal ? [...categoriesRaw].reverse() : categoriesRaw } : undefined,
  }
}
