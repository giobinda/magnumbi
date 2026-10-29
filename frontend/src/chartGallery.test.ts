import { describe, expect, it } from 'vitest'
import { GALLERY } from './chartGallery'

// what each chart needs from its query (README: "A tile's type, and what it needs")
const NEEDS: Record<string, [dims: number, measures: number]> = {
  table: [1, 1], bar: [1, 1], hbar: [1, 1], line: [1, 1], area: [1, 1], pie: [1, 1], scatter: [1, 2],
  kpi: [0, 1], trend: [1, 1], combo: [1, 2], pivot: [2, 1], heatmap: [2, 1], treemap: [1, 1],
  funnel: [1, 1], waterfall: [1, 1], histogram: [1, 1], gauge: [0, 1],
}

describe('the chart gallery', () => {
  it('offers every chart, each with what it is for', () => {
    expect(GALLERY.map(c => c.type).sort()).toEqual(Object.keys(NEEDS).sort())
    for (const c of GALLERY) expect(c.use.length, c.type).toBeGreaterThan(10)
  })

  it('shows each with made-up data that suits it', () => {
    for (const c of GALLERY) {
      const r = c.sample()
      const [dims, measures] = NEEDS[c.type]
      expect(r.rows.length, c.type).toBeGreaterThan(0)
      expect(r.columns.filter(x => x.kind === 'dimension').length, c.type).toBeGreaterThanOrEqual(dims)
      expect(r.columns.filter(x => x.kind === 'measure').length, c.type).toBeGreaterThanOrEqual(measures)
      for (const row of r.rows) expect(row.length, c.type).toBe(r.columns.length)
    }
  })
})
