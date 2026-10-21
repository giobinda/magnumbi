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

/** Explore's field search: the name, the label, or any synonym. */
export function fieldMatches(f: { name: string; label?: string | null; synonyms?: string[] }, query: string): boolean {
  const q = query.trim().toLowerCase()
  return !q || [f.name, f.label ?? '', ...(f.synonyms ?? [])].some(x => x.toLowerCase().includes(q))
}

/** Items typed with commas: trimmed, once each (whatever the case). */
export function parseList(text: string): string[] {
  const out: string[] = []
  for (const x of text.split(',').map(s => s.trim()).filter(Boolean)) {
    if (!out.some(o => o.toLowerCase() === x.toLowerCase())) out.push(x)
  }
  return out
}

/** Tags that ask for care with the data (shown next to the field). */
export const SENSITIVE = ['pii', 'personal', 'confidential', 'restricted']
export const sensitive = (tags?: string[]) => (tags ?? []).filter(t => SENSITIVE.includes(t))

/** A field's tooltip: its formula, description, synonyms, examples, terms and tags, a line each. */
export function fieldTitle(f: {
  formula?: string | null; description?: string | null; synonyms?: string[]; examples?: string[]; terms?: string[]; tags?: string[]
}): string | undefined {
  const lines = [
    f.formula ? `= ${f.formula}` : '',
    f.description ?? '',
    f.synonyms?.length ? `Also called: ${f.synonyms.join(', ')}` : '',
    f.examples?.length ? `For example: ${f.examples.join('; ')}` : '',
    f.terms?.length ? `Glossary: ${f.terms.join(', ')}` : '',
    f.tags?.length ? `Tags: ${f.tags.join(', ')}` : '',
  ].filter(Boolean)
  return lines.length ? lines.join('\n') : undefined
}

/** A model's queries over a period, in words. */
export function describeUsage(u: { days: number; queries: number; people: number; last: string | null }): string {
  if (!u.queries) return `Not queried in the last ${u.days} days`
  const n = (x: number, one: string, many: string) => `${x} ${x === 1 ? one : many}`
  return `${n(u.queries, 'query', 'queries')} by ${n(u.people, 'person', 'people')} in the last ${u.days} days`
}

/** A field's glossary terms with their definitions (null: no longer in the glossary). */
export function definitions(terms: string[] | undefined, glossary: { name: string; definition: string }[]) {
  return (terms ?? []).map(t => {
    const found = glossary.find(g => g.name.toLowerCase() === t.toLowerCase())
    return { name: found?.name ?? t, definition: found?.definition ?? null }
  })
}
