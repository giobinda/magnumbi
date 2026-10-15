import { describe, expect, it } from 'vitest'
import type { Column, QueryResult } from './api'
import { buildOption } from './chartOption'

const dim: Column = { name: 'day', field: 'day', label: 'Day', kind: 'dimension', type: 'date', grain: 'day' }
const measure = (name: string, format = 'number', compare?: 'previous' | 'change'): Column => ({
  name: compare ? `${name}__${compare}` : name, field: name, label: name, kind: 'measure', type: 'number', format,
  compare: compare ?? null, base: compare ? name : null,
})
const result = (columns: Column[], rows: (string | number)[][]): QueryResult => ({ columns, rows, stats: {}, sql: null })
const palette = ['#111', '#222', '#333']

type Series = { name: string; yAxisIndex?: number }
const axes = (o: Record<string, unknown>) => (Array.isArray(o.yAxis) ? o.yAxis.length : 1)
const axisOf = (o: Record<string, unknown>) => Object.fromEntries((o.series as Series[]).map(s => [s.name, s.yAxisIndex ?? 0]))

describe('value axes', () => {
  it('gives two measures two scales, even in the same format', () => {
    const r = result([dim, measure('trips'), measure('temperature')], [['2026-07-01', 110695, 28.8], ['2026-07-02', 107249, 32.4]])
    const o = buildOption(r, 'line', {}, palette)
    expect(axes(o)).toBe(2)
    expect(axisOf(o)).toEqual({ trips: 0, temperature: 1 })
  })

  it('puts a previous-period series on its own measure\'s scale', () => {
    const r = result(
      [dim, measure('trips'), measure('trips', 'number', 'previous'), measure('trips', 'number', 'change'),
       measure('temperature'), measure('temperature', 'number', 'previous'), measure('temperature', 'number', 'change')],
      [['2026-07-01', 110695, 108349, 0.02, 28.8, 24.9, 0.16]],
    )
    const series = buildOption(r, 'line', {}, palette).series as Series[]
    expect(series.map(s => s.yAxisIndex)).toEqual([0, 0, 1, 1])
  })

  it('keeps one scale for one measure, stacked charts and horizontal bars', () => {
    const one = result([dim, measure('trips')], [['2026-07-01', 1]])
    expect(axes(buildOption(one, 'line', {}, palette))).toBe(1)
    const two = result([dim, measure('a'), measure('b')], [['2026-07-01', 1, 2]])
    expect(axes(buildOption(two, 'bar', { stacked: true }, palette))).toBe(1)
    expect(Array.isArray(buildOption(two, 'hbar', {}, palette).xAxis)).toBe(false)
  })

  it('names each scale after its measure when there are two', () => {
    const r = result([dim, measure('trips'), measure('temperature')], [['2026-07-01', 1, 2]])
    const y = buildOption(r, 'line', {}, palette).yAxis as { name?: string }[]
    expect(y.map(a => a.name)).toEqual(['trips', 'temperature'])
  })
})
