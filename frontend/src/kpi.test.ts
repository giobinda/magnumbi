import { describe, expect, it } from 'vitest'
import { comparison } from './kpi'

describe('a KPI against the comparison period', () => {
  it('shows the change and the earlier value when there is one', () => {
    expect(comparison(0.12, 1200)).toEqual({ change: 0.12, previous: 1200 })
    expect(comparison(-0.5, 3)).toEqual({ change: -0.5, previous: 3 })
  })

  it('says there was nothing then, rather than "– vs 0" or "– vs –"', () => {
    expect(comparison(null, 0)).toBe('none')
    expect(comparison(null, null)).toBe('none')
    expect(comparison(Number.NaN, 0)).toBe('none')
  })

  it('keeps an earlier value with no change worth a ratio (from zero to something)', () => {
    expect(comparison(null, 5)).toEqual({ change: null, previous: 5 })
  })
})
