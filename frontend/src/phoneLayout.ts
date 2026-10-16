import type { Layout } from 'react-grid-layout'
import type { TileLayout } from './api'

/** On a phone, tiles stack in reading order: numbers (KPIs) two per row, everything else full
 *  width. For display only: a dashboard's saved layout is the desktop one. */
export function phoneLayout(tiles: { id: string; type: string; layout: TileLayout }[]): Layout[] {
  const ordered = [...tiles].sort((a, b) => a.layout.y - b.layout.y || a.layout.x - b.layout.x)
  const out: Layout[] = []
  let y = 0
  let half: Layout | null = null // a number tile waiting for a neighbour on its row
  for (const t of ordered) {
    if (t.type === 'kpi') {
      if (half) { out.push({ i: t.id, x: 6, y: half.y, w: 6, h: half.h }); y = half.y + half.h; half = null; continue }
      half = { i: t.id, x: 0, y, w: 6, h: t.layout.h }
      out.push(half)
      continue
    }
    if (half) { y = half.y + half.h; half = null }
    out.push({ i: t.id, x: 0, y, w: 12, h: t.layout.h })
    y += t.layout.h
  }
  return out
}
