import { useMemo, useState } from 'react'
import type { ChartType, Column, QueryResult, Value } from '../api'
import { formatChange, formatNumber, formatValue } from '../format'
import EChart from './EChart'
import { buildOption } from '../chartOption'
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
