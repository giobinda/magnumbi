import { describe, expect, it } from 'vitest'
import type { Column, QueryResult, Value } from './api'
import { buildOption } from './chartOption'
import { histogramBins, pivot, trendSummary, waterfallSeries } from './chartMore'

const dim = (name: string, type = 'string'): Column => ({ name, field: name, label: name, kind: 'dimension', type })
const mea = (name: string, additive = true, format = 'number'): Column => ({ name, field: name, label: name, kind: 'measure', type: 'number', format, additive })
const res = (columns: Column[], rows: Value[][]): QueryResult => ({ columns, rows, stats: {}, sql: null })
const palette = ['#111111', '#222222', '#333333']
type S = { type: string; name: string; data: unknown[]; stack?: string }

describe('pivot table', () => {
  it('spreads a second field across columns, with totals when the measure adds up', () => {
    const r = res([dim('region'), dim('year'), mea('revenue')], [['EMEA', '2025', 10], ['EMEA', '2026', 5], ['AMER', '2026', 7]])
    const p = pivot(r)!
    expect(p.rowKeys).toEqual(['AMER', 'EMEA'])
    expect(p.colKeys).toEqual(['2025', '2026'])
    expect(p.cells).toEqual([[null, 7], [10, 5]])
    expect(p.rowTotals).toEqual([7, 15])
    expect(p.colTotals).toEqual([10, 12])
    expect(p.total).toBe(22)
    // an average doesn't add up: no totals
    expect(pivot(res([dim("region"), dim("year"), mea("aov", false)], r.rows))!.total).toBeNull()
  })
})

describe('waterfall', () => {
  it('floats each step on the running total and ends with the total', () => {
    const w = waterfallSeries([10, -3, 5])
    expect(w.base).toEqual([0, 7, 7, 0])
    expect(w.up).toEqual([10, '-', 5, '-'])
    expect(w.down).toEqual(['-', 3, '-', '-'])
    expect(w.total).toEqual(['-', '-', '-', 12])
  })
})

describe('histogram', () => {
  it('counts values into equal bins, weighted by the measure', () => {
    const bins = histogramBins([1, 2, 3, 9, 10], [1, 1, 2, 1, 5], 3)
    expect(bins.map(b => [b.from, b.to, b.count])).toEqual([[1, 4, 4], [4, 7, 0], [7, 10, 6]])
  })
})

describe('trend', () => {
  it('shows the latest value, its change on the one before, and the series in time order', () => {
    const r = res([dim('day', 'date'), mea('trips')], [['2026-08-03', 120], ['2026-08-01', 100], ['2026-08-02', 80]])
    const t = trendSummary(r)!
    expect(t.value).toBe(120)
    expect(t.change).toBeCloseTo(0.5)
    expect(t.points).toEqual([100, 80, 120])
  })
})

describe('new ECharts types', () => {
  it('heatmap: one field across, one down, colour by value', () => {
    const r = res([dim('hour', 'number'), dim('weekday'), mea('trips')], [[0, 'Mon', 5], [1, 'Mon', 7], [0, 'Tue', 2]])
    const o = buildOption(r, 'heatmap', {}, palette) as { series: { type: string; data: number[][] }[]; visualMap: { min: number; max: number } }
    expect(o.series[0].type).toBe('heatmap')
    expect(o.series[0].data).toEqual([[0, 0, 5], [1, 0, 7], [0, 1, 2]])
    expect([o.visualMap.min, o.visualMap.max]).toEqual([2, 7])
  })

  it('combo: bars for the first measure, lines for the others', () => {
    const r = res([dim('day', 'date'), mea('trips'), mea('temperature', false)], [['2026-08-01', 100, 25]])
    const o = buildOption(r, 'combo', {}, palette) as { series: S[] }
    expect(o.series.map(s => s.type)).toEqual(['bar', 'line'])
  })

  it('100% stacks: each category\'s values as shares', () => {
    const r = res([dim('region'), dim('year'), mea('revenue')], [['EMEA', '2025', 30], ['EMEA', '2026', 10], ['AMER', '2025', 5], ['AMER', '2026', 5]])
    const o = buildOption(r, 'bar', { stacked: true, percent: true }, palette) as { series: S[] }
    expect(o.series.map(s => s.data)).toEqual([[0.75, 0.5], [0.25, 0.5]])
    expect((o as unknown as { yAxis: { max?: number } }).yAxis.max).toBe(1) // shares stop at 100%
  })

  it('funnel keeps the query\'s order; treemap nests a second field', () => {
    const f = buildOption(res([dim('step'), mea('n')], [['Visit', 100], ['Cart', 40], ['Paid', 10]]), 'funnel', {}, palette) as { series: { sort: string; data: { name: string }[] }[] }
    expect(f.series[0].sort).toBe('none')
    expect(f.series[0].data.map(d => d.name)).toEqual(['Visit', 'Cart', 'Paid'])
    const t = buildOption(res([dim('region'), dim('store'), mea('n')], [['EMEA', 'S1', 3], ['EMEA', 'S2', 2], ['AMER', 'S3', 4]]), 'treemap', {}, palette) as { series: { data: { name: string; value: number; children?: unknown[] }[] }[] }
    expect(t.series[0].data.map(d => [d.name, d.value, d.children?.length])).toEqual([['AMER', 4, 1], ['EMEA', 5, 2]])
  })

  it('gauge: towards the target, or a round number above the value', () => {
    const r = res([mea('revenue', true, 'currency')], [[730]])
    const g = (o: Record<string, unknown>) => (buildOption(r, 'gauge', o, palette) as { series: { max: number; data: { value: number }[] }[] }).series[0]
    expect(g({ target: 1000 }).max).toBe(1000)
    expect(g({}).max).toBe(1000)
    expect(g({}).data[0].value).toBe(730)
  })
})

describe('category order', () => {
  it('puts weekdays and months in calendar order, everything else by value', async () => {
    const { orderKeys } = await import('./chartMore')
    expect(orderKeys(['Wednesday', 'Monday', 'Sunday', 'Friday'])).toEqual(['Monday', 'Wednesday', 'Friday', 'Sunday'])
    expect(orderKeys(['Mar', 'jan', 'Feb'])).toEqual(['jan', 'Feb', 'Mar'])
    expect(orderKeys(['10', '9', 'b', 'a'])).toEqual(['9', '10', 'a', 'b'])
  })
})
