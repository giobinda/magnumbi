import { describe, expect, it } from 'vitest'
import { cacheLabel, uncachedAccepted } from './share'

describe('public links', () => {
  it('says how long a link keeps its results, in words', () => {
    expect(cacheLabel(300)).toBe('5 minutes')
    expect(cacheLabel(3600)).toBe('1 hour')
    expect(cacheLabel(6 * 3600)).toBe('6 hours')
    expect(cacheLabel(86400)).toBe('1 day')
    expect(cacheLabel(0)).toBe('no cache (live)')
  })

  it('accepts the risks of no cache only when ticked and the dashboard id is typed', () => {
    expect(uncachedAccepted({ understood: false, typed: 'sales' }, 'sales')).toBe(false)
    expect(uncachedAccepted({ understood: true, typed: 'sale' }, 'sales')).toBe(false)
    expect(uncachedAccepted({ understood: true, typed: ' sales ' }, 'sales')).toBe(true)
  })
})
