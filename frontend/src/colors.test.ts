import { describe, expect, it } from 'vitest'
import { contrast, onColor, slug } from './colors'

describe('colours', () => {
  it('measures contrast like WCAG', () => {
    expect(contrast('#000000', '#ffffff')).toBeCloseTo(21, 0)
    expect(contrast('#ffffff', '#ffffff')).toBeCloseTo(1, 5)
    expect(contrast('#c8102e', '#fffdf8')).toBeGreaterThan(5.5)
  })
  it('picks readable text on a colour', () => {
    expect(onColor('#0055a5')).toBe('#ffffff')
    expect(onColor('#ffd400')).toBe('#000000')
  })
  it('makes theme ids from names', () => {
    expect(slug('ACME Corp — Brand 2026!')).toBe('acme-corp-brand-2026')
    expect(slug('  ')).toBe('')
  })
})
