/** An organisation's own model, as the server keeps it (no SQL: columns and aggregations). */
export interface DimDef { name: string; label?: string | null; type: string; hidden?: boolean }
export interface MeasureDef { name: string; label?: string | null; type: string; sql?: string | null; format?: string; hidden?: boolean }
export interface ModelDef {
  name: string; label?: string | null; description?: string | null; connection: string; table?: string | null
  dimensions: DimDef[]; measures: MeasureDef[]
  /** drill paths: a name, then its dimensions in drilling order (country › region › store) */
  hierarchies?: Record<string, string[]>
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
export function modelFrom(meta: { name: string; label: string; connection: string; table: string }, rows: FieldRow[], count: MeasureDef,
  paths: Record<string, string[]> = {}): ModelDef {
  const kept = rows.filter(r => r.include)
  // a drill path keeps the dimensions still in the model; one left a step short goes
  const dims = new Set(kept.filter(r => r.role === 'dimension').map(r => r.column))
  const hierarchies = Object.fromEntries(Object.entries(paths)
    .map(([h, p]) => [h, p.filter(d => dims.has(d))] as const).filter(([, p]) => p.length >= 2))
  return {
    ...meta,
    hierarchies,
    dimensions: kept.filter(r => r.role === 'dimension').map(r => ({ name: r.column, label: r.label, type: r.type })),
    measures: [count, ...kept.filter(r => r.role === 'measure')
      .map(r => ({ name: r.column, label: r.label, type: r.type, sql: r.column, format: r.format ?? 'number' }))],
  }
}
