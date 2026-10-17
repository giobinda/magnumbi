import { describe, expect, it } from 'vitest'
import { suggestChart } from './suggest'

const date = { temporal: true }, text = { temporal: false }

describe('chart suggestion', () => {
  it('picks a chart from the fields chosen', () => {
    expect(suggestChart([], 1)).toBe('kpi')
    expect(suggestChart([], 2)).toBe('kpi')
    expect(suggestChart([date], 1)).toBe('line')
    expect(suggestChart([date], 2)).toBe('combo')
    expect(suggestChart([text], 1)).toBe('bar')
    expect(suggestChart([text, date], 1)).toBe('line')
    expect(suggestChart([text, text], 1)).toBe('heatmap')
    expect(suggestChart([text, text], 2)).toBe('table')
    expect(suggestChart([text, text, text], 1)).toBe('table')
    expect(suggestChart([text], 0)).toBe('table')
    expect(suggestChart([], 0)).toBeNull()
  })
})
