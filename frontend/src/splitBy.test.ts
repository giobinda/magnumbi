import { describe, expect, it } from 'vitest'
import { splitBy, splitNote, splitOf } from './splitBy'

describe('Split by', () => {
  it('is the second dimension of a chart', () => {
    expect(splitOf(['day', 'borough'])).toBe('borough')
    expect(splitOf(['day'])).toBe('')
  })

  it('sets, changes or removes it, keeping the first', () => {
    expect(splitBy(['day'], 'borough')).toEqual(['day', 'borough'])
    expect(splitBy(['day', 'borough'], 'payment')).toEqual(['day', 'payment'])
    expect(splitBy(['day', 'borough', 'zone'], '')).toEqual(['day'])
    // the same field on both: nothing to split
    expect(splitBy(['day'], 'day')).toEqual(['day'])
    expect(splitBy(['pickup_time:day'], 'pickup_time')).toEqual(['pickup_time:day'])
  })

  it('says why it does nothing with more than one measure', () => {
    expect(splitNote(['day', 'borough'], ['trips'])).toBeNull()
    expect(splitNote(['day', 'borough'], ['trips', 'fare'])).toMatch(/one measure/)
    expect(splitNote(['day'], ['trips', 'fare'])).toBeNull()
  })
})
