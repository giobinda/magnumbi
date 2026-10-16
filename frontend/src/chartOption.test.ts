import { describe, expect, it } from 'vitest'
import type { Column, QueryResult } from './api'
import { buildOption, shade } from './chartOption'

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

describe('period colours', () => {
  it('shades a colour towards another', () => {
    expect(shade('#000000', '#ffffff', 0.5)).toBe('#808080')
    expect(shade('#336699', '#ffffff', 0)).toBe('#336699')
    expect(shade('#fff', '#000', 1)).toBe('#000000')
    expect(shade('rgb(1, 2, 3)', '#ffffff', 0.5)).toBe('rgb(1, 2, 3)') // not hex: left as is
  })

  it('draws the previous period in a different shade of its measure\'s colour', () => {
    const r = result([dim, measure('trips'), measure('trips', 'number', 'previous'), measure('trips', 'number', 'change')],
      [['2026-07-01', 110695, 108349, 0.02]])
    const [cur, prev] = buildOption(r, 'line', {}, ['#0f6cbd']).series as { color: string }[]
    expect(cur.color).toBe('#0f6cbd')
    expect(prev.color).not.toBe(cur.color)
    expect(prev.color).toBe(shade('#0f6cbd', '#ffffff', 0.5)) // towards the background (white outside a browser)
  })
})

describe('a second dimension', () => {
  const borough: Column = { name: 'borough', field: 'borough', label: 'Borough', kind: 'dimension', type: 'string' }
  const condition: Column = { name: 'condition', field: 'condition', label: 'Condition', kind: 'dimension', type: 'string' }
  type Data = { name: string; data: (number | null)[] }

  it('splits into one series per value, also with a period comparison', () => {
    const rows: (string | number)[][] = [['Manhattan', 'Clear', 10], ['Manhattan', 'Rain', 3], ['Queens', 'Clear', 2]]
    const plain = buildOption(result([borough, condition, measure('trips')], rows), 'bar', { stacked: true }, palette)
    const compared = buildOption(
      result([borough, condition, measure('trips'), measure('trips', 'number', 'previous'), measure('trips', 'number', 'change')],
        rows.map(r => [...r, 1, 0.5])),
      'bar', { stacked: true }, palette,
    )
    for (const o of [plain, compared]) {
      const series = (o.series as Data[]).map(s => [s.name, s.data])
      expect(series).toEqual([['Clear', [10, 2]], ['Rain', [3, null]]])
    }
  })

  it('orders the series by value, so a value keeps its colour from chart to chart', () => {
    const byTrips = buildOption(result([borough, condition, measure('trips')], [['Manhattan', 'Rain', 9], ['Manhattan', 'Clear', 3], ['Manhattan', 'Cloudy', 1]]), 'bar', {}, palette)
    const byHour = buildOption(result([borough, condition, measure('trips')], [['Manhattan', 'Cloudy', 9], ['Manhattan', 'Rain', 3], ['Manhattan', 'Clear', 1]]), 'line', {}, palette)
    for (const o of [byTrips, byHour]) expect((o.series as Data[]).map(s => s.name)).toEqual(['Clear', 'Cloudy', 'Rain'])
    const hours = buildOption(result([borough, condition, measure('trips')], [['x', '10', 1], ['x', '9', 1]]), 'bar', {}, palette)
    expect((hours.series as Data[]).map(s => s.name)).toEqual(['9', '10'])
  })
})
