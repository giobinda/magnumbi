// What uses a dataset (a connection, a model) about to be deleted, from each model's lineage, and
// what to delete with it: only what's ticked, as what stays works again once a new one of the same
// name is back.
import type { Blend, DashboardSummary, Lineage, ModelInfo } from './api'

export type Target = { kind: 'connection' | 'model'; name: string }
export type Item = { name: string; label: string; deletable: boolean }
export type DashboardUse = { id: string; title: string; used: number; total: number | null }
export type Uses = {
  models: Item[]; blends: Item[]; dashboards: DashboardUse[]
  /** scheduled reports run on these dashboards */
  reports: boolean
  /** those whose uses couldn't be read */
  unknown: string[]
}

const modelsOf = (t: Target, models: ModelInfo[]) =>
  models.filter(m => !m.blend && (t.kind === 'model' ? m.name === t.name : m.connection === t.name))

const blendsOf = (names: string[], models: ModelInfo[]) =>
  models.filter(m => m.blend && [m.blend.base, ...m.blend.joins.map(j => j.model)].some(n => names.includes(n)))

/** The models and blends whose lineage says what the dataset's deletion reaches. */
export function namesToCheck(t: Target, models: ModelInfo[]): string[] {
  const own = modelsOf(t, models).map(m => m.name)
  return [...own, ...blendsOf(own, models).map(m => m.name)]
}

export function usesOf(
  t: Target,
  models: ModelInfo[],
  lineages: Record<string, Lineage | null>,
  here: { models: string[]; blends: Blend[]; dashboards: DashboardSummary[] },
): Uses {
  const on = modelsOf(t, models)
  const blends = blendsOf(on.map(m => m.name), models)
  const tiles = new Map<string, { title: string; tiles: Set<string> }>()
  for (const l of Object.values(lineages)) {
    for (const d of l?.dashboards ?? []) {
      const seen = tiles.get(d.id) ?? { title: d.title, tiles: new Set<string>() }
      d.tiles.forEach(x => seen.tiles.add(x))
      tiles.set(d.id, seen)
    }
  }
  return {
    // deleting a model: it isn't one of what uses it
    models: t.kind === 'model' ? [] : on.map(m => ({ name: m.name, label: m.label ?? m.name, deletable: here.models.includes(m.name) })),
    blends: blends.map(m => {
      const b = here.blends.find(x => x.name === m.name)
      return { name: m.name, label: b?.label || m.label || m.name, deletable: Boolean(b?.editable) }
    }),
    dashboards: [...tiles].map(([id, d]) => ({
      id, title: d.title, used: d.tiles.size, total: here.dashboards.find(x => x.id === id)?.tiles ?? null,
    })),
    reports: Object.values(lineages).some(l => (l?.reports ?? 0) > 0),
    unknown: Object.entries(lineages).filter(([, l]) => !l).map(([n]) => n),
  }
}

export type Step = { kind: 'dashboard' | 'blend' | 'model' | 'connection'; name: string }

/** What to delete, in an order that leaves nothing half done: what uses others first, the dataset
 *  itself last. `ticked`: "dashboard:<id>", "blend:<name>", "model:<name>". */
export function deleteSteps(t: Target, ticked: Set<string>): Step[] {
  const of = (kind: Step['kind']) =>
    [...ticked].filter(k => k.startsWith(`${kind}:`)).map(k => ({ kind, name: k.slice(kind.length + 1) }))
  return [...of('dashboard'), ...of('blend'), ...of('model'), t]
}
