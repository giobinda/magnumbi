// The in-app chat's helpers: what to suggest asking, and how an answer's queries are shown.
import { fieldLabel } from './api'
import type { ChartTile, ModelInfo, Proposal, Query } from './api'
import { suggestChart } from './suggest'

const temporal = (t: string) => t === 'date' || t === 'time' || t === 'datetime'

/** A few questions to start with: on certified models first, never deprecated ones, and the glossary. */
export function suggestions(models: ModelInfo[], glossary: { name: string }[]): string[] {
  const usable = models
    .filter(m => m.governance?.status !== 'deprecated' && m.measures.length > 0)
    .sort((a, b) => Number(b.governance?.status === 'certified') - Number(a.governance?.status === 'certified'))
  const out: string[] = []
  const m = usable[0]
  if (m) {
    const measure = m.measures[0].label ?? m.measures[0].name
    if (m.dimensions.some(d => temporal(d.type))) out.push(`How has ${measure} changed over the last 3 months?`)
    const d = m.dimensions.find(x => x.type === 'string')
    if (d) out.push(`Which ${d.label ?? d.name} has the highest ${measure}?`)
  }
  if (glossary[0]) out.push(`What does ${glossary[0].name} mean?`)
  return out.slice(0, 4)
}

/** A query from an answer, as a tile that suits it. */
export function chartTile(query: Query, i: number): ChartTile {
  const dims = (query.dimensions ?? []).map(d => ({ temporal: d.includes(':') }))
  return {
    id: `chat-${i}`, title: null, type: suggestChart(dims, (query.measures ?? []).length) ?? 'table', drill: null,
    layout: { x: 0, y: 0, w: 6, h: 5 }, options: {}, query,
  }
}

/** What an answer's query is based on: the model (and its status) and the fields, by their labels. */
export function basedOn(query: Query, models: ModelInfo[]) {
  const m = models.find(x => x.name === query.model)
  const refs = [...(query.dimensions ?? []), ...(query.measures ?? [])]
  return { model: m?.label ?? query.model, status: m?.governance?.status ?? null, fields: refs.map(r => fieldLabel(m, r)) }
}

/** The question sent with an attached file: what the person typed, or to add it. */
export function attachQuestion(typed: string, filename: string): string {
  return typed.trim() || `Add ${filename} as a data source.`
}

/** What the link to a confirmed proposal's result says. */
export function madeLabel(p: Proposal): string {
  return p.tool === 'add_file_model' ? 'Open in Explore' : 'Open the dashboard'
}

/** Where to see what a confirmed proposal made: the dashboard it saved, its report's dashboard, a file's model. */
export function madeLink(p: Proposal): string | null {
  if (p.status !== 'applied') return null
  if (p.tool === 'add_file_model') return typeof p.args.name === 'string' ? `/explore?model=${encodeURIComponent(p.args.name)}` : null
  const id = p.tool === 'save_dashboard' ? p.args.id : p.tool === 'create_report' ? p.args.dashboard : null
  return typeof id === 'string' ? `/dashboards/${encodeURIComponent(id)}` : null
}
