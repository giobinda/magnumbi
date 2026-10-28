import { describe, expect, it } from 'vitest'
import type { Dashboard, Tile } from './api'
import { withTile } from './tileEdit'

const tile = (id: string, title: string, extra: Partial<Tile> = {}): Tile =>
  ({ id, title, type: 'bar', layout: { x: 0, y: 0, w: 6, h: 5 }, query: { model: 'sales', measures: ['revenue'] }, ...extra } as Tile)

describe('editing one tile from the dashboard', () => {
  const dash = { title: 'D', tiles: [tile('a', 'A', { tab: 'Q1', layout: { x: 6, y: 2, w: 3, h: 4 } }), tile('b', 'B')] } as unknown as Dashboard

  it('changes that tile only, where it is', () => {
    const next = withTile(dash, tile('a', 'A, by region'))
    expect(next.tiles.map(t => t.title)).toEqual(['A, by region', 'B'])
    // its tab and its place on the grid stay (the editor knows neither)
    expect(next.tiles[0].tab).toBe('Q1')
    expect(next.tiles[0].layout).toEqual({ x: 6, y: 2, w: 3, h: 4 })
  })

  it('leaves the dashboard as it was', () => {
    withTile(dash, tile('a', 'changed'))
    expect(dash.tiles[0].title).toBe('A')
  })
})
