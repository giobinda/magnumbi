// The formula editor's helpers. The server parses and checks formulas (backend/src/formula.rs);
// this only helps people write them.
import { fieldName, parseAdhoc } from './api'
import type { DimType, Formula, ModelInfo, Query } from './api'

/** The functions a formula may call, with what each inserts ("|": where the cursor goes). */
export const FUNCTIONS: { name: string; template: string; help: string }[] = [
  { name: 'if', template: 'if(| > 0, , )', help: 'if(condition, then, else)' },
  { name: 'round', template: 'round(|, 2)', help: 'round(number, decimal places)' },
  { name: 'coalesce', template: 'coalesce(|, 0)', help: 'the first value that isn\'t empty' },
  { name: 'abs', template: 'abs(|)', help: 'the number without its sign' },
  { name: 'least', template: 'least(|, )', help: 'the smallest of the numbers' },
  { name: 'greatest', template: 'greatest(|, )', help: 'the largest of the numbers' },
]

/** A per-row formula may also work on text. */
export const ROW_FUNCTIONS = [
  ...FUNCTIONS,
  { name: 'lower', template: 'lower(|)', help: 'the text in small letters' },
  { name: 'upper', template: 'upper(|)', help: 'the text in capitals' },
  { name: 'concat', template: "concat(|, ' ', )", help: 'texts (and numbers) joined' },
]

/** Calculations on a chart's results (run after the query, on ScramDB): only in a chart. */
export const RESULT_FUNCTIONS = [
  { name: 'running_sum', template: 'running_sum(|)', help: 'the total so far, along the first date (or dimension)' },
  { name: 'moving_avg', template: 'moving_avg(|, 3)', help: 'the average of the last N rows, along the first date' },
  { name: 'previous', template: 'previous(|)', help: 'the value on the row before, along the first date' },
  { name: 'rank', template: 'rank(|)', help: '1 for the largest, across the other dimensions' },
  { name: 'percent_of_total', template: 'percent_of_total(|)', help: 'the share of the total, across the other dimensions' },
]

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
