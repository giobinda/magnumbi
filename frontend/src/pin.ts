// Put a chart on a dashboard, below what's there (Explore's and the chat's "Add to dashboard").
import { api } from './api'
import type { ChartTile } from './api'

export async function addTile(dashboardId: string, tile: ChartTile) {
  const { dashboard } = await api.dashboard(dashboardId)
  const bottom = Math.max(0, ...dashboard.tiles.map(t => t.layout.y + t.layout.h))
  let id = (tile.title || tile.query.measures?.[0] || 'tile').toLowerCase().replace(/[^a-z0-9]+/g, '_')
  while (dashboard.tiles.some(t => t.id === id)) id += '_2'
  const small = tile.type === 'kpi'
  dashboard.tiles.push({ ...tile, id, layout: { x: 0, y: bottom, w: small ? 3 : 6, h: small ? 2 : 5 } })
  await api.saveDashboard(dashboardId, dashboard)
}
