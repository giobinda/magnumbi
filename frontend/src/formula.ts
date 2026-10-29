// The formula editor's helpers. The server parses and checks formulas (backend/src/formula.rs);
// this only helps people write them.
import { fieldName, parseAdhoc } from './api'
import type { DimType, Formula, ModelInfo, Query } from './api'

/** A function a formula may call: what it inserts ("|": where the cursor goes), its group in the
 *  editor, and whether it's one of the harder ones, under Advanced (closed by default). */
export type FormulaFunction = { name: string; template: string; help: string; group: string; advanced?: boolean }

/** The functions a formula may call. */
export const FUNCTIONS: FormulaFunction[] = [
  { name: 'if', template: 'if(| > 0, , )', help: 'if(condition, then, else)', group: 'Logic' },
  { name: 'round', template: 'round(|, 2)', help: 'round(number, decimal places)', group: 'Numbers' },
  { name: 'coalesce', template: 'coalesce(|, 0)', help: 'the first value that isn\'t empty', group: 'Logic', advanced: true },
  { name: 'abs', template: 'abs(|)', help: 'the number without its sign', group: 'Numbers' },
  { name: 'least', template: 'least(|, )', help: 'the smallest of the numbers', group: 'Numbers', advanced: true },
  { name: 'greatest', template: 'greatest(|, )', help: 'the largest of the numbers', group: 'Numbers', advanced: true },
]

/** A per-row formula may also work on text. */
export const ROW_FUNCTIONS: FormulaFunction[] = [
  ...FUNCTIONS,
  { name: 'lower', template: 'lower(|)', help: 'the text in small letters', group: 'Text' },
  { name: 'upper', template: 'upper(|)', help: 'the text in capitals', group: 'Text' },
  { name: 'concat', template: "concat(|, ' ', )", help: 'texts (and numbers) joined', group: 'Text' },
]

/** Calculations on a chart's results (run after the query, on ScramDB): only in a chart. */
export const RESULT_FUNCTIONS: FormulaFunction[] = [
  { name: 'running_sum', template: 'running_sum(|)', help: 'the total so far, along the first date (or dimension)', group: 'Compared with other rows' },
  { name: 'moving_avg', template: 'moving_avg(|, 3)', help: 'the average of the last N rows, along the first date', group: 'Compared with other rows' },
  { name: 'previous', template: 'previous(|)', help: 'the value on the row before, along the first date', group: 'Compared with other rows' },
  { name: 'rank', template: 'rank(|)', help: '1 for the largest, across the other dimensions', group: 'Compared with other rows' },
  { name: 'percent_of_total', template: 'percent_of_total(|)', help: 'the share of the total, across the other dimensions', group: 'Compared with other rows' },
]

const GROUPS = ['Numbers', 'Logic', 'Text', 'Dates', 'Compared with other rows']

/** The functions by group, in the editor's order; the advanced ones only when Advanced is open. */
export function functionGroups(fns: FormulaFunction[], advanced: boolean): { group: string; functions: FormulaFunction[] }[] {
  return GROUPS.map(group => ({ group, functions: fns.filter(f => f.group === group && (advanced || !f.advanced)) }))
    .filter(g => g.functions.length)
}

type Measure = { ref: string; label: string; format?: Formula['format'] }

/** Calculations on a chart's measure, in plain words, each a formula people can open and change:
 *  how they learn the language. `others`: the chart's other measures, to divide by. */
export function quickCalcs(m: Measure, others: Omit<Measure, 'format'>[], taken: string[]): { menu: string; formula: Formula }[] {
  const amount = m.format ?? 'number'
  const x = m.ref
  // the menu, the label's ending, the name's ending (as code would call it), the formula, its format
  const calcs: [string, string, string, string, Formula['format']][] = [
    ['Share of total', 'share of total', 'share of total', `percent_of_total(${x})`, 'percent'],
    ['Change from previous', 'change', 'change', `${x} - previous(${x})`, amount],
    ['% change from previous', '% change', 'pct change', `(${x} - previous(${x})) / previous(${x})`, 'percent'],
    ['Running total', 'running total', 'running total', `running_sum(${x})`, amount],
    ['Average of last 3', 'average of last 3', 'avg last 3', `moving_avg(${x}, 3)`, amount],
    ['Rank', 'rank', 'rank', `rank(${x})`, 'number'],
  ]
  const used = [...taken]
  const formula = (label: string, named: string, expr: string, format: Formula['format']): Formula => {
    const name = nameFor(named, used)
    used.push(name)
    return { name, label, expr, format }
  }
  return [
    ...calcs.map(([menu, what, named, expr, format]) =>
      ({ menu, formula: formula(`${m.label}, ${what}`, `${m.label} ${named}`, expr, format) })),
    ...others.map(o => {
      const label = `${m.label} per ${o.label}`
      return { menu: `Divided by ${o.label}`, formula: formula(label, label, `${x} / ${o.ref}`, 'number') }
    }),
  ]
}

type Chart = {
  model: { measures: { name: string; format?: Formula['format'] }[]; dimensions: { name: string }[] }
  measures: string[]
  formulas: Formula[]
  /** how many dimensions the chart has */
  dimensions: number
  label: (ref: string) => string
}

/** A chart measure's quick calculations: on measures and the chart's formulas, never on a
 *  calculation on results (they can't use one another); comparing rows needs a dimension. */
export function calcsInChart(ref: string, chart: Chart): { menu: string; formula: Formula }[] {
  const own = (r: string) => chart.formulas.find(f => f.name === r)
  const calculable = (r: string) => !usesResults(own(r)?.expr ?? '')
  if (!calculable(ref)) return []
  const format = own(ref)?.format ?? chart.model.measures.find(m => m.name === ref)?.format ?? undefined
  const others = chart.measures.filter(o => o !== ref && calculable(o)).map(o => ({ ref: o, label: chart.label(o) }))
  const taken = [...chart.model.dimensions, ...chart.model.measures, ...chart.formulas].map(x => x.name)
  return quickCalcs({ ref, label: chart.label(ref), format }, others, taken)
    .filter(c => chart.dimensions > 0 || !usesResults(c.formula.expr))
}

type Store = { getItem(key: string): string | null; setItem(key: string, value: string): void }
const ADVANCED_KEY = 'magnum.formula.advanced'

/** Whether this person keeps the editor's Advanced open: closed until they open it. */
export function advancedOpen(storage: Store): boolean {
  try { return storage.getItem(ADVANCED_KEY) === '1' } catch { return false }
}

export function rememberAdvanced(storage: Store, open: boolean) {
  try { storage.setItem(ADVANCED_KEY, open ? '1' : '0') } catch { /* only a convenience */ }
}

/** Whether a formula calculates on the chart's results (so it can't be shared, nor be a model's). */
export function usesResults(expr: string): boolean {
  return RESULT_FUNCTIONS.some(f => new RegExp(`\\b${f.name}\\s*\\(`, 'i').test(expr))
}

export const OPERATORS = ['+', '-', '*', '/', '(', ')', '>', '<', '=', 'and', 'or']

/** A field name from a label ("Revenue per sale" -> revenue_per_sale), not one of `taken`. */
export function nameFor(label: string, taken: string[]): string {
  let base = label.toLowerCase().replace(/[^a-z0-9]+/g, '_').replace(/^_+|_+$/g, '') || 'formula'
  if (/^\d/.test(base)) base = `f_${base}`
  let name = base
  for (let n = 2; taken.includes(name); n++) name = `${base}_${n}`
  return name
}

/** `snippet` in place of text[start, end]; a word gets a space where it would run into another. */
export function insertAt(text: string, start: number, end: number, snippet: string): { text: string; cursor: number } {
  const mark = snippet.indexOf('|')
  let s = snippet.replace('|', '')
  const before = text.slice(0, start), after = text.slice(end)
  const word = /[A-Za-z0-9_)]$/
  if (/^[A-Za-z0-9_]/.test(s) && word.test(before)) s = ' ' + s
  const cursor = before.length + (mark >= 0 ? mark + (s.length - snippet.length + 1) : s.length)
  return { text: before + s + after, cursor }
}

/** The query with the formula added (or `replacing` the one of that name), as one of its measures. */
export function withFormula(q: Query, f: Formula, replacing?: string): Query {
  const formulas = q.formulas ?? [], measures = q.measures ?? []
  if (replacing && formulas.some(x => x.name === replacing)) {
    return {
      ...q,
      formulas: formulas.map(x => (x.name === replacing ? f : x)),
      measures: measures.map(m => (m === replacing ? f.name : m)),
    }
  }
  // a per-row formula is a field: the chart uses it as it likes (grouped by, summed, filtered)
  return { ...q, measures: f.per_row ? measures : [...measures, f.name], formulas: [...formulas, f] }
}

/** The query without the formula, nor anything that used it. */
export function withoutFormula(q: Query, name: string): Query {
  const out: Query = {
    ...q,
    measures: (q.measures ?? []).filter(m => m !== name && parseAdhoc(m)?.field !== name),
    formulas: (q.formulas ?? []).filter(f => f.name !== name),
  }
  if (q.dimensions) out.dimensions = q.dimensions.filter(d => fieldName(d) !== name)
  if (q.filters) out.filters = q.filters.filter(f => fieldName(f.field) !== name)
  return out
}

/** The model with the query's per-row formulas among its dimensions (`types`: what each gives). */
export function withRowFields(model: ModelInfo, formulas: Formula[], types: Record<string, DimType>): ModelInfo {
  const rows = formulas.filter(f => f.per_row)
  if (!rows.length) return model
  return {
    ...model,
    dimensions: [...model.dimensions, ...rows.map(f => ({
      name: f.name, label: f.label ?? undefined, type: types[f.name] ?? 'string', formula: f.expr, own: true,
    }))],
  }
}
