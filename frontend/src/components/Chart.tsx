import { comparison } from '../kpi'
import { useMemo, useState } from 'react'
import type { ChartType, Column, QueryResult, Value } from '../api'
import { formatChange, formatNumber, formatValue } from '../format'
import EChart from './EChart'
import { buildOption } from '../chartOption'
import { pivot, trendSummary } from '../chartMore'
import { useTheme } from '../theme'



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
  if (type === 'pivot') return <PivotTable result={result} />
  if (type === 'trend') return <Trend result={result} />
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
        const against = ci >= 0 ? comparison(row[ci], pi >= 0 ? row[pi] : null) : null
        return (
          <div key={c.name} className="kpi-item">
            <div className="kpi-value">{typeof row[i] === 'number' ? formatNumber(row[i] as number, c.format, true) : formatValue(row[i], c)}</div>
            {measures.length > 1 && <div className="kpi-label">{c.label}</div>}
            {against === 'none' && <div className="kpi-compare muted">no data in the comparison period</div>}
            {against && against !== 'none' && (
              <div className="kpi-compare">
                <Change value={against.change} higherIsBetter={c.higher_is_better} />
                <span className="muted"> vs {typeof against.previous === 'number' ? formatNumber(against.previous, c.format, true) : '–'}</span>
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

/** Rows by the first field, columns by the second; totals when the measure adds up; each cell
 *  tinted by its value. */
function PivotTable({ result }: { result: QueryResult }) {
  const p = pivot(result)
  if (!p) return <div className="empty">A pivot table needs 2 dimensions + 1 measure</div>
  const f = (v: number | null) => (v === null ? '' : formatNumber(v, p.measure.format))
  const max = Math.max(1e-9, ...p.cells.flat().map(v => Math.abs(v ?? 0)))
  const tint = (v: number | null) => (v === null ? undefined : { background: `color-mix(in srgb, var(--accent) ${Math.round((Math.abs(v) / max) * 28)}%, transparent)` })
  return (
    <div className="table-wrap">
      <table className="data pivot">
        <thead>
          <tr><th>{p.rowLabel} <span className="muted">╲ {p.colLabel}</span></th>
            {p.colKeys.map(c => <th key={c} className="num">{c}</th>)}
            {p.rowTotals && <th className="num">Total</th>}</tr>
        </thead>
        <tbody>
          {p.rowKeys.map((r, ri) => (
            <tr key={r}><td>{r}</td>
              {p.cells[ri].map((v, ci) => <td key={ci} className="num" style={tint(v)}>{f(v)}</td>)}
              {p.rowTotals && <td className="num total">{f(p.rowTotals[ri])}</td>}</tr>
          ))}
          {p.colTotals && (
            <tr className="total"><td>Total</td>{p.colTotals.map((v, ci) => <td key={ci} className="num">{f(v)}</td>)}<td className="num">{f(p.total)}</td></tr>
          )}
        </tbody>
      </table>
    </div>
  )
}

/** The latest value, its change on the value before, and a sparkline of the series. */
function Trend({ result }: { result: QueryResult }) {
  const { theme } = useTheme()
  const t = useMemo(() => trendSummary(result), [result])
  const option = useMemo(() => t && ({
    animation: false, grid: { left: 0, right: 0, top: 4, bottom: 0 },
    xAxis: { type: 'category', show: false, boundaryGap: false, data: t.points.map((_, i) => i) },
    yAxis: { type: 'value', show: false, scale: true },
    series: [{ type: 'line', data: t.points, smooth: 0.3, showSymbol: false, lineStyle: { width: 2, color: theme.palette[0] },
      areaStyle: { color: theme.palette[0], opacity: 0.12 } }],
  }), [t, theme])
  if (!t) return <div className="empty">A trend needs a date dimension + 1 measure</div>
  return (
    <div className="trend">
      <div className="kpi-value">{t.value === null ? '–' : formatNumber(t.value, t.measure.format, true)}</div>
      <div className="kpi-compare"><Change value={t.change} higherIsBetter={t.measure.higher_is_better} /><span className="muted"> on the previous point</span></div>
      <div className="trend-spark"><EChart option={option!} /></div>
    </div>
  )
}
