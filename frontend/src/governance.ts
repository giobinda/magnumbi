// Data governance in the UI: a model's owner, stewards and status (backend/src/governance.rs).
import type { ModelInfo, Standing } from './api'

export const STATUSES = ['certified', 'draft', 'deprecated'] as const

/** A model in a picker: its name, and its status when it has one. */
export function modelOption(m: ModelInfo): string {
  const status = m.governance?.status
  return status ? `${m.label} · ${status}` : m.label
}

/** The deprecated models the tiles use, once each, with why. */
export function deprecatedIn(tiles: { query?: { model: string } | null }[], models: ModelInfo[]) {
  const names = [...new Set(tiles.map(t => t.query?.model).filter(Boolean))] as string[]
  return names
    .map(n => models.find(m => m.name === n))
    .filter((m): m is ModelInfo => m?.governance?.status === 'deprecated')
    .map(m => ({ name: m.name, label: m.label, note: m.governance?.note ?? null }))
}

/** The owner and Org admins decide stewards and status. */
export const mayDecide = (s: Standing) => s === 'admin' || s === 'owner'
/** Only Org admins give a model a new owner. */
export const mayHandOver = (s: Standing) => s === 'admin'
