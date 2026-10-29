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
  { name: 'in', template: "in(|, '', '')", help: 'whether a value is one of those listed: in(region, \'EMEA\', \'APAC\')', group: 'Logic' },
  { name: 'case', template: "case(|, '', '', 'other')", help: 'one answer per value, then one for the rest: case(region, \'EMEA\', \'Europe\', \'Other\')', group: 'Logic', advanced: true },
  { name: 'floor', template: 'floor(|)', help: 'the whole number at or below: floor(2.7) is 2', group: 'Numbers' },
  { name: 'ceil', template: 'ceil(|)', help: 'the whole number at or above: ceil(2.1) is 3', group: 'Numbers' },
  { name: 'mod', template: 'mod(|, 2)', help: 'what remains after dividing: mod(7, 2) is 1', group: 'Numbers', advanced: true },
  { name: 'power', template: 'power(|, 2)', help: 'a number to a power: power(3, 2) is 9', group: 'Numbers', advanced: true },
  { name: 'sqrt', template: 'sqrt(|)', help: 'the square root (empty below zero)', group: 'Numbers', advanced: true },
  { name: 'ln', template: 'ln(|)', help: 'the natural logarithm (empty at zero and below)', group: 'Numbers', advanced: true },
  { name: 'log10', template: 'log10(|)', help: 'the logarithm in base 10: log10(1000) is 3', group: 'Numbers', advanced: true },
  { name: 'exp', template: 'exp(|)', help: 'e to the power of the number', group: 'Numbers', advanced: true },
]

/** A formula on totals may also add up the rows that meet a condition. */
export const TOTAL_FUNCTIONS: FormulaFunction[] = [
  ...FUNCTIONS,
  { name: 'sum_if', template: "sum_if(|, region = '')", help: "the total of a column over the rows that meet a condition: sum_if(amount, region = 'EMEA')", group: 'Totals with a condition' },
  { name: 'count_if', template: "count_if(| = '')", help: "how many rows meet a condition: count_if(region = 'EMEA')", group: 'Totals with a condition' },
  { name: 'avg_if', template: "avg_if(|, region = '')", help: "the average of a column over the rows that meet a condition", group: 'Totals with a condition' },
  { name: 'min_if', template: "min_if(|, region = '')", help: 'the smallest value over the rows that meet a condition', group: 'Totals with a condition' },
  { name: 'max_if', template: "max_if(|, region = '')", help: 'the largest value over the rows that meet a condition', group: 'Totals with a condition' },
  { name: 'count_distinct_if', template: "count_distinct_if(|, region = '')", help: 'how many different values over the rows that meet a condition', group: 'Totals with a condition', advanced: true },
  { name: 'stddev', template: 'stddev(|)', help: 'how spread out a column\'s values are (standard deviation of a sample)', group: 'Spread', advanced: true },
  { name: 'variance', template: 'variance(|)', help: 'the variance of a column\'s values (of a sample)', group: 'Spread', advanced: true },
]

/** A per-row formula may also work on text. */
export const ROW_FUNCTIONS: FormulaFunction[] = [
  ...FUNCTIONS,
  { name: 'lower', template: 'lower(|)', help: 'the text in small letters', group: 'Text' },
  { name: 'upper', template: 'upper(|)', help: 'the text in capitals', group: 'Text' },
  { name: 'concat', template: "concat(|, ' ', )", help: 'texts (and numbers) joined', group: 'Text' },
  { name: 'contains', template: "contains(|, '')", help: 'whether a text has another in it (capitals aside)', group: 'Text' },
  { name: 'starts_with', template: "starts_with(|, '')", help: 'whether a text begins with another (capitals aside)', group: 'Text' },
  { name: 'ends_with', template: "ends_with(|, '')", help: 'whether a text ends with another (capitals aside)', group: 'Text' },
  { name: 'length', template: 'length(|)', help: 'how many characters a text has', group: 'Text' },
  { name: 'trim', template: 'trim(|)', help: 'the text without spaces at either end', group: 'Text' },
  { name: 'left', template: 'left(|, 3)', help: 'the first characters of a text', group: 'Text' },
  { name: 'right', template: 'right(|, 3)', help: 'the last characters of a text', group: 'Text' },
  { name: 'replace', template: "replace(|, '', '')", help: 'a text with one piece replaced by another, everywhere', group: 'Text' },
  { name: 'substring', template: 'substring(|, 1, 3)', help: 'a piece of a text: from a character (1: the first), so many characters', group: 'Text' },
  { name: 'position', template: "position(|, '')", help: 'where a text is in another (1: at the start; 0: not in it)', group: 'Text', advanced: true },
  { name: 'split_part', template: "split_part(|, '-', 1)", help: 'one part of a text split by a separator: split_part(code, \'-\', 2)', group: 'Text', advanced: true },
  { name: 'to_number', template: 'to_number(|)', help: 'a text as a number (empty if it isn\'t one)', group: 'Text' },
  { name: 'to_text', template: 'to_text(|)', help: 'a number or a date as text', group: 'Text' },
  { name: 'year', template: 'year(|)', help: 'the year of a date, e.g. 2026', group: 'Dates' },
  { name: 'quarter', template: 'quarter(|)', help: 'the quarter of a date, 1 to 4', group: 'Dates' },
  { name: 'month', template: 'month(|)', help: 'the month of a date, 1 to 12', group: 'Dates' },
  { name: 'week', template: 'week(|)', help: 'the week of the year (weeks start on Monday)', group: 'Dates' },
  { name: 'day', template: 'day(|)', help: 'the day of the month, 1 to 31', group: 'Dates' },
  { name: 'weekday', template: 'weekday(|)', help: 'the day of the week: 1 for Monday to 7 for Sunday', group: 'Dates' },
  { name: 'hour', template: 'hour(|)', help: 'the hour of a time, 0 to 23', group: 'Dates' },
  { name: 'today', template: 'today()|', help: "today's date", group: 'Dates' },
  { name: 'days_between', template: 'days_between(|, today())', help: 'the days from one date to another', group: 'Dates' },
  { name: 'date_trunc', template: "date_trunc(|, 'month')", help: "the start of its hour, day, week, month, quarter or year", group: 'Dates', advanced: true },
  { name: 'date_add', template: "date_add(|, 30, 'day')", help: 'a date moved by a number of hours, days, weeks, months or years (minus: back)', group: 'Dates', advanced: true },
  { name: 'date_diff', template: "date_diff(|, today(), 'month')", help: 'calendar days, months or years from one date to another', group: 'Dates', advanced: true },
  { name: 'hours_between', template: 'hours_between(|, )', help: 'the hours from one time to another (1.5: an hour and a half)', group: 'Dates' },
  { name: 'minutes_between', template: 'minutes_between(|, )', help: 'the minutes from one time to another', group: 'Dates' },
  { name: 'format_date', template: "format_date(|, 'YYYY-MM')", help: 'a date as text: YYYY, MM, DD, HH, MI, Mon (Mar), Dy (Sun)', group: 'Dates' },
  { name: 'fiscal_year', template: 'fiscal_year(|, 4)', help: 'the fiscal year, starting in the month given, named after the year it ends in', group: 'Dates', advanced: true },
  { name: 'fiscal_quarter', template: 'fiscal_quarter(|, 4)', help: 'the quarter of the fiscal year starting in the month given', group: 'Dates', advanced: true },
]

/** What a per-row formula gives (its type, as the server says), in words. */
export function gives(type: DimType | string | undefined): string {
  switch (type) {
    case 'number': return 'a number'
    case 'string': return 'text'
    case 'boolean': return 'yes or no'
    case 'date': return 'a date'
    case 'time': case 'datetime': return 'a date and time'
    default: return ''
  }
}

/** Calculations on a chart's results (run after the query, on ScramDB): only in a chart. */
export const RESULT_FUNCTIONS: FormulaFunction[] = [
  { name: 'running_sum', template: 'running_sum(|)', help: 'the total so far, along the first date (or dimension)', group: 'Compared with other rows' },
  { name: 'moving_avg', template: 'moving_avg(|, 3)', help: 'the average of the last N rows, along the first date', group: 'Compared with other rows' },
  { name: 'previous', template: 'previous(|)', help: 'the value on the row before, along the first date', group: 'Compared with other rows' },
  { name: 'rank', template: 'rank(|)', help: '1 for the largest, across the other dimensions', group: 'Compared with other rows' },
  { name: 'percent_of_total', template: 'percent_of_total(|)', help: 'the share of the total, across the other dimensions', group: 'Compared with other rows' },
  { name: 'running_avg', template: 'running_avg(|)', help: 'the average so far, along the first date', group: 'Compared with other rows' },
  { name: 'next', template: 'next(|)', help: 'the value on the row after, along the first date', group: 'Compared with other rows' },
  { name: 'difference', template: 'difference(|)', help: 'the change from the row before, along the first date', group: 'Compared with other rows' },
  { name: 'percent_change', template: 'percent_change(|)', help: 'the change from the row before, as a percentage', group: 'Compared with other rows' },
  { name: 'dense_rank', template: 'dense_rank(|)', help: 'like rank, without gaps after ties (1, 1, 2)', group: 'Compared with other rows', advanced: true },
  { name: 'first', template: 'first(|)', help: 'the value on the first row, along the first date', group: 'Compared with other rows' },
  { name: 'last', template: 'last(|)', help: 'the value on the last row, along the first date', group: 'Compared with other rows' },
  { name: 'ntile', template: 'ntile(|, 4)', help: 'which of N equal groups, largest first: ntile(revenue, 4) gives quartiles', group: 'Compared with other rows', advanced: true },
  { name: 'along', template: ', along(|)', help: 'after the value: the dimension to go along, e.g. running_sum(revenue, along(sold_at))', group: 'Compared with other rows', advanced: true },
  { name: 'within', template: ', within(|)', help: 'after the value: the dimensions to start again for, e.g. within(region); within() for the whole chart', group: 'Compared with other rows', advanced: true },
]

/** Along or within each of the chart's dimensions (and the whole chart), to click after a
 *  calculation's value: under Advanced. */
export function windowChips(dims: { ref: string; label: string }[]): { label: string; insert: string }[] {
  const chips = dims.flatMap(d => {
    const field = d.ref.split(':')[0]
    return [
      { label: `along ${d.label}`, insert: `, along(${field})` },
      { label: `within ${d.label}`, insert: `, within(${field})` },
    ]
  })
  return dims.length ? [...chips, { label: 'within the whole chart', insert: ', within()' }] : []
}

/** Adding up rows that can't be added up again after a blend's join. */
const NOT_ON_BLENDS = ['count_distinct_if', 'stddev', 'variance']

/** The functions the editor offers: per row or on totals, on a chart's results in a chart, and on
 *  a blend only the totals that add up again after the join (its models' rows are added up first). */
export function editorFunctions({ perRow, inChart, blend }: { perRow: boolean; inChart: boolean; blend: boolean }): FormulaFunction[] {
  if (perRow) return ROW_FUNCTIONS
  const totals = blend ? TOTAL_FUNCTIONS.filter(f => !NOT_ON_BLENDS.includes(f.name)) : TOTAL_FUNCTIONS
  return inChart ? [...totals, ...RESULT_FUNCTIONS] : totals
}

const GROUPS = ['Numbers', 'Logic', 'Text', 'Dates', 'Totals with a condition', 'Spread', 'Compared with other rows']

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
    ['Change from previous', 'change', 'change', `difference(${x})`, amount],
    ['% change from previous', '% change', 'pct change', `percent_change(${x})`, 'percent'],
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

/** A chart measure's quick calculations: on measures and the chart's formulas, calculations on
 *  results included (they may use one another); comparing rows needs a dimension. */
export function calcsInChart(ref: string, chart: Chart): { menu: string; formula: Formula }[] {
  const own = (r: string) => chart.formulas.find(f => f.name === r)
  const format = own(ref)?.format ?? chart.model.measures.find(m => m.name === ref)?.format ?? undefined
  // calculations on results too: they're calculated before the ones using them
  const others = chart.measures.filter(o => o !== ref).map(o => ({ ref: o, label: chart.label(o) }))
  const taken = [...chart.model.dimensions, ...chart.model.measures, ...chart.formulas].map(x => x.name)
  return quickCalcs({ ref, label: chart.label(ref), format }, others, taken)
    .filter(c => chart.dimensions > 0 || !usesResults(c.formula.expr, calculationNames(chart.formulas)))
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
export function usesResults(expr: string, calcs: string[] = []): boolean {
  return RESULT_FUNCTIONS.some(f => new RegExp(`\\b${f.name}\\s*\\(`, 'i').test(expr))
    // or uses one of the chart's calculations on results, calculated before it
    || calcs.some(c => new RegExp(`(^|[^\\w.])${c.replace(/[.]/g, '\\.')}($|[^\\w.(])`).test(expr))
}

/** The chart's formulas that calculate on its results: calling a function on results, or using
 *  one that does (as results.rs's `calculations`). */
export function calculationNames(formulas: Pick<Formula, 'name' | 'expr' | 'per_row'>[]): string[] {
  const names: string[] = []
  for (let grew = true; grew;) {
    grew = false
    for (const f of formulas) {
      if (f.per_row || names.includes(f.name) || !usesResults(f.expr, names)) continue
      names.push(f.name)
      grew = true
    }
  }
  return names
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
