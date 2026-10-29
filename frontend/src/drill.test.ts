import { describe, expect, it } from 'vitest'
import { drillable } from './drill'

describe('whether a chart can drill', () => {
  const path = ['borough', 'zone']
  it('only where a field shown has a level below it', () => {
    expect(drillable(['borough'], path)).toBe(true)
    // the last level, or a field outside the path: nothing below
    expect(drillable(['zone'], path)).toBe(false)
    expect(drillable(['payment'], path)).toBe(false)
    expect(drillable(['payment'], [])).toBe(false)
  })

  it('dates drill to a finer grain, down to days', () => {
    expect(drillable(['pickup_at:month'], [])).toBe(true)
    expect(drillable(['pickup_at:year'], [])).toBe(true)
    expect(drillable(['pickup_at:day'], [])).toBe(false)
    expect(drillable(['pickup_at:hour'], [])).toBe(false)
    // not the field the period owns
    expect(drillable(['pickup_at:month'], [], 'pickup_at')).toBe(false)
    expect(drillable(['payment', 'pickup_at:month'], [], 'dropoff_at')).toBe(true)
  })
})
