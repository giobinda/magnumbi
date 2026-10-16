import { describe, expect, it } from 'vitest'
import { phoneLayout } from './phoneLayout'

const tile = (id: string, type: string, x: number, y: number, w: number, h: number) => ({ id, type, layout: { x, y, w, h } })

describe('phone layout', () => {
  it('stacks tiles in reading order: numbers two per row, everything else full width', () => {
    const tiles = [
      tile('chart', 'line', 0, 2, 8, 5), tile('side', 'pie', 8, 2, 4, 5),
      tile('a', 'kpi', 0, 0, 3, 2), tile('b', 'kpi', 3, 0, 3, 2), tile('c', 'kpi', 6, 0, 3, 2),
    ]
    expect(phoneLayout(tiles).map(l => [l.i, l.x, l.y, l.w, l.h])).toEqual([
      ['a', 0, 0, 6, 2], ['b', 6, 0, 6, 2], ['c', 0, 2, 6, 2],
      ['chart', 0, 4, 12, 5], ['side', 0, 9, 12, 5],
    ])
  })
})
