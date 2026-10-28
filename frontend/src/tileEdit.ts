import type { Dashboard, Tile } from './api'

/** The dashboard with one tile changed (edited on its own, from the dashboard): where it was, on
 *  its tab and at its place on the grid, which the tile editor doesn't change. */
export function withTile(dash: Dashboard, tile: Tile): Dashboard {
  return { ...dash, tiles: dash.tiles.map(t => (t.id === tile.id ? { ...tile, tab: t.tab, layout: t.layout } : t)) }
}
