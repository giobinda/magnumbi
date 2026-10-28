/** An organisation's own model, as the server keeps it (no SQL: columns and aggregations). */
export interface DimDef {
  name: string; label?: string | null; type: string; hidden?: boolean
  /** a lookup's column (its `as`), and the column when it isn't named as the field */
  from?: string; column?: string
}
/** A table the model looks up: one row per key, matched on [the model's column, its column]. */
export interface JoinDef { table: string; as: string; on: [string, string][] }
export interface MeasureDef { name: string; label?: string | null; type: string; sql?: string | null; format?: string; hidden?: boolean }
export interface ModelDef {
  name: string; label?: string | null; description?: string | null; connection: string; table?: string | null
  dimensions: DimDef[]; measures: MeasureDef[]
  /** drill paths: a name, then its dimensions in drilling order (country › region › store) */
  hierarchies?: Record<string, string[]>
  joins?: JoinDef[]
}

/** One of the table's columns in the editor: in the model or not, as what. */
export interface FieldRow {
  column: string; include: boolean; role: 'dimension' | 'measure'; type: string; label: string; format?: string
}

export const DIMENSION_TYPES = ['string', 'number', 'boolean', 'date', 'time', 'datetime']
export const MEASURE_TYPES = ['sum', 'avg', 'min', 'max', 'count_distinct']

/** The column a measure aggregates (its `sql`, unquoted). */
const columnOf = (m: MeasureDef) => (m.sql ?? '').replace(/["`]/g, '')

/** A row per column of the table (the proposal has them all): as proposed, or as `saved` has it
 *  (its left-out columns unticked). The count of rows isn't a column: see modelFrom. */
export function rowsFor(proposal: ModelDef, saved?: ModelDef): FieldRow[] {
  const proposed: FieldRow[] = [
    ...proposal.dimensions.map(d => ({ column: d.name, include: true, role: 'dimension' as const, type: d.type, label: d.label ?? d.name })),
    ...proposal.measures.filter(m => m.sql).map(m => ({ column: columnOf(m), include: true, role: 'measure' as const, type: m.type, label: m.label ?? m.name })),
  ]
  if (!saved) return proposed
  return proposed.map(r => {
    const d = saved.dimensions.find(x => x.name === r.column)
    if (d) return { ...r, include: true, role: 'dimension', type: d.type, label: d.label ?? d.name }
    const m = saved.measures.find(x => x.sql && columnOf(x) === r.column)
    if (m) return { ...r, include: true, role: 'measure', type: m.type, label: m.label ?? m.name, format: m.format }
    return { ...r, include: false }
  })
}

/** The model the rows describe: the count of rows first, then the columns kept (measures name
 *  their column bare: the server quotes it for its connection). */
/** Drill paths through the dimensions still there; one left a step short goes. */
export function keepPaths(paths: Record<string, string[]>, dims: string[]): Record<string, string[]> {
  const have = new Set(dims)
  return Object.fromEntries(Object.entries(paths)
    .map(([h, p]) => [h, p.filter(d => have.has(d))] as const).filter(([, p]) => p.length >= 2))
}

export function modelFrom(meta: { name: string; label: string; connection: string; table: string }, rows: FieldRow[], count: MeasureDef,
  paths: Record<string, string[]> = {}): ModelDef {
  const kept = rows.filter(r => r.include)
  return {
    ...meta,
    hierarchies: keepPaths(paths, kept.filter(r => r.role === 'dimension').map(r => r.column)),
    dimensions: kept.filter(r => r.role === 'dimension').map(r => ({ name: r.column, label: r.label, type: r.type })),
    measures: [count, ...kept.filter(r => r.role === 'measure')
      .map(r => ({ name: r.column, label: r.label, type: r.type, sql: r.column, format: r.format ?? 'number' }))],
  }
}

/** A lookup in the editor: its table, what matches, and its columns as fields (ticked or not). */
export interface LookupDef extends JoinDef {
  fields: { column: string; include: boolean; label: string; type: string }[]
}

/** The columns that most likely match: the lookup's id with the model's <lookup>_id, else a
 *  column both have. ['', ''] when nothing stands out. */
export function suggestMatch(mine: string[], theirs: string[], alias: string): [string, string] {
  if (theirs.includes('id')) {
    const key = mine.find(c => c === `${alias}_id`) ?? mine.find(c => c.endsWith('_id') && alias.startsWith(c.slice(0, -3)))
    if (key) return [key, 'id']
  }
  const same = mine.find(c => c !== 'id' && theirs.includes(c))
  return same ? [same, same] : ['', '']
}

/** The model with its lookups: their joins, and their ticked columns as dimensions, named after
 *  the column, or <lookup>_<column> when the model already has that name. */
export function withLookups(m: ModelDef, lookups: LookupDef[]): ModelDef {
  const taken = new Set([...m.dimensions.map(d => d.name), ...m.measures.map(x => x.name)])
  const dims: DimDef[] = []
  for (const l of lookups) {
    for (const f of l.fields.filter(f => f.include)) {
      const name = taken.has(f.column) ? `${l.as}_${f.column}` : f.column
      taken.add(name)
      dims.push({ name, label: f.label, type: f.type, from: l.as, ...(name === f.column ? {} : { column: f.column }) })
    }
  }
  const joins = lookups.map(({ table, as, on }) => ({ table, as, on }))
  return { ...m, dimensions: [...m.dimensions, ...dims], joins }
}

/** The lookups of a saved model, for editing (their fields as ticked; untick to leave one out). */
export function lookupsOf(m: ModelDef): LookupDef[] {
  return (m.joins ?? []).map(j => ({
    ...j,
    fields: m.dimensions.filter(d => d.from === j.as)
      .map(d => ({ column: d.column ?? d.name, include: true, label: d.label ?? d.name, type: d.type })),
  }))
}

/** The model without its lookups: its own table's fields only. */
export function withoutLookups(m: ModelDef): ModelDef {
  return { ...m, dimensions: m.dimensions.filter(d => !d.from), joins: [] }
}
