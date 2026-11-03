import { describe, expect, it } from 'vitest'
import { isLong, matches, matchesOrBlank, nextActive, pickable, placePopover } from './search'

describe('searching a long list', () => {
  it('finds every word typed, in any order, capitals and accents aside', () => {
    expect(matches('', 'anything')).toBe(true)
    expect(matches('  ', 'anything')).toBe(true)
    expect(matches('trips', 'NYC taxi trips')).toBe(true)
    expect(matches('TAXI nyc', 'NYC taxi trips')).toBe(true)
    expect(matches('taxi bus', 'NYC taxi trips')).toBe(false)
    expect(matches('caffe', 'Caffè Milano')).toBe(true)
    // across all that's given: a label, a name, a hint
    expect(matches('pickup_at time', 'Pickup time', 'pickup_at')).toBe(true)
    expect(matches('x', null, undefined)).toBe(false)
  })

  it('a row just added, still blank, stays in sight whatever is searched', () => {
    expect(matchesOrBlank('ana', '')).toBe(true)
    expect(matchesOrBlank('ana', '  ', null)).toBe(true)
    expect(matchesOrBlank('ana', 'ben@acme.example')).toBe(false)
    expect(matchesOrBlank('ben', 'ben@acme.example')).toBe(true)
  })

  it('a list is long past 8', () => {
    expect(isLong(8)).toBe(false)
    expect(isLong(9)).toBe(true)
  })

  it('narrows a picker\'s options, keeping their order', () => {
    const options = [
      { value: 'a', label: 'Sales', group: 'Models' },
      { value: 'b', label: 'Stock', hint: 'warehouse', group: 'Models' },
      { value: 'c', label: 'Sales × weather', group: 'Blends' },
    ]
    expect(pickable(options, 'sales').map(o => o.value)).toEqual(['a', 'c'])
    expect(pickable(options, 'warehouse').map(o => o.value)).toEqual(['b'])
    // the group's name finds its options too
    expect(pickable(options, 'blends').map(o => o.value)).toEqual(['c'])
    expect(pickable(options, '').length).toBe(3)
  })

  it('moves through the options with the arrows, skipping what can\'t be picked', () => {
    const options = [{ value: 'a', label: 'A' }, { value: 'b', label: 'B', disabled: true }, { value: 'c', label: 'C' }]
    expect(nextActive(options, 0, 1)).toBe(2)
    expect(nextActive(options, 2, -1)).toBe(0)
    // stays at the ends
    expect(nextActive(options, 2, 1)).toBe(2)
    expect(nextActive(options, 0, -1)).toBe(0)
    expect(nextActive([], 0, 1)).toBe(0)
  })

  it('opens where it fits: below, or above near the bottom, as wide as it needs and within the window', () => {
    const window = { width: 1000, height: 800 }
    expect(placePopover({ left: 100, top: 100, bottom: 130, width: 400 }, window))
      .toEqual({ left: 100, width: 400, top: 134, maxHeight: 360 })
    // narrower than a useful list: wider
    expect(placePopover({ left: 100, top: 100, bottom: 130, width: 120 }, window).width).toBe(320)
    // near the bottom: above it
    expect(placePopover({ left: 100, top: 700, bottom: 730, width: 400 }, window))
      .toEqual({ left: 100, width: 400, bottom: 104, maxHeight: 360 })
    // near the right edge: moved in
    expect(placePopover({ left: 900, top: 100, bottom: 130, width: 90 }, window).left).toBe(672)
  })
})
