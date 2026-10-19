import { describe, expect, it } from 'vitest'
import { onTab, pickTab } from './tabs'

const tiles = [{ id: 'a' }, { id: 'b', tab: 'Detail' }, { id: 'c', tab: 'Gone' }]

describe('tabs', () => {
  it('shows every tile when there are no tabs', () => {
    expect(onTab(tiles, [], null).map(t => t.id)).toEqual(['a', 'b', 'c'])
  })

  it('puts tiles without a (known) tab on the first one', () => {
    expect(onTab(tiles, ['Overview', 'Detail'], 'Overview').map(t => t.id)).toEqual(['a', 'c'])
    expect(onTab(tiles, ['Overview', 'Detail'], 'Detail').map(t => t.id)).toEqual(['b'])
  })

  it('opens the tab asked for, else the first', () => {
    expect(pickTab(['A', 'B'], 'B')).toBe('B')
    expect(pickTab(['A', 'B'], 'Z')).toBe('A')
    expect(pickTab(['A', 'B'], null)).toBe('A')
    expect(pickTab([], 'A')).toBeNull()
  })
})
