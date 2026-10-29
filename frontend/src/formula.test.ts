import { describe, expect, it } from 'vitest'
import { FUNCTIONS, RESULT_FUNCTIONS, editorFunctions, windowChips, ROW_FUNCTIONS, TOTAL_FUNCTIONS, advancedOpen, calcsInChart, functionGroups, gives, quickCalcs, rememberAdvanced, usesResults, insertAt, nameFor, withFormula, withRowFields, withoutFormula } from './formula'
import type { ModelInfo } from './api'

describe('formulas', () => {
  it('names a formula after its label, avoiding names taken', () => {
    expect(nameFor('Revenue per sale', [])).toBe('revenue_per_sale')
    expect(nameFor('Margin %', ['margin'])).toBe('margin_2')
    expect(nameFor('2024 growth', [])).toBe('f_2024_growth')
    expect(nameFor('  ', [])).toBe('formula')
  })

  it('inserts at the cursor, leaving it where typing goes on', () => {
    expect(insertAt('revenue  cost', 8, 8, '-')).toEqual({ text: 'revenue - cost', cursor: 9 })
    // a function's template puts the cursor inside it
    expect(insertAt('', 0, 0, 'round(|, 2)')).toEqual({ text: 'round(, 2)', cursor: 6 })
    // a selection is replaced
    expect(insertAt('a + b', 4, 5, 'units')).toEqual({ text: 'a + units', cursor: 9 })
    // spaces around a field name when it runs into other text
    expect(insertAt('revenue/', 8, 8, 'units')).toEqual({ text: 'revenue/units', cursor: 13 })
    expect(insertAt('revenue', 7, 7, 'units')).toEqual({ text: 'revenue units', cursor: 13 })
  })

  it('knows the functions the server allows', () => {
    expect(FUNCTIONS.map(f => f.name)).toEqual(['if', 'round', 'coalesce', 'abs', 'least', 'greatest', 'in', 'case'])
    expect(ROW_FUNCTIONS.map(f => f.name)).toEqual(['if', 'round', 'coalesce', 'abs', 'least', 'greatest', 'in', 'case',
      'lower', 'upper', 'concat', 'contains', 'starts_with', 'ends_with', 'length', 'trim', 'left', 'right', 'replace',
      'year', 'quarter', 'month', 'week', 'day', 'weekday', 'hour', 'today', 'days_between', 'date_trunc', 'date_add', 'date_diff'])
  })

  it('adds a formula to a query as a measure, and takes it away', () => {
    const q = { model: 'sales', measures: ['revenue'] }
    const f = { name: 'per_sale', label: 'Per sale', expr: 'revenue / transactions' }
    const added = withFormula(q, f)
    expect(added).toEqual({ model: 'sales', measures: ['revenue', 'per_sale'], formulas: [f] })
    // edited in place
    const edited = withFormula(added, { ...f, expr: 'revenue / units' }, 'per_sale')
    expect(edited.formulas).toEqual([{ ...f, expr: 'revenue / units' }])
    expect(edited.measures).toEqual(['revenue', 'per_sale'])
    // renamed: the measure follows
    expect(withFormula(added, { ...f, name: 'avg_sale' }, 'per_sale').measures).toEqual(['revenue', 'avg_sale'])
    expect(withoutFormula(added, 'per_sale')).toEqual({ model: 'sales', measures: ['revenue'], formulas: [] })
  })

  it('keeps per-row formulas out of the measures, and takes them out of everything when removed', () => {
    const line = { name: 'line', expr: 'amount * qty', per_row: true }
    const q = withFormula({ model: 'sales', dimensions: ['region'], measures: ['revenue'] }, line)
    expect(q.measures).toEqual(['revenue'])
    expect(q.formulas).toEqual([line])
    const used = { ...q, dimensions: ['region', 'line'], measures: ['revenue', 'sum(line)'], filters: [{ field: 'line', op: 'gt' as const, values: [1] }] }
    expect(withoutFormula(used, 'line')).toEqual({ model: 'sales', dimensions: ['region'], measures: ['revenue'], filters: [], formulas: [] })
  })

  it('lists per-row formulas among the model\'s dimensions', () => {
    const model = { name: 'sales', label: 'Sales', connection: 'w', connection_type: 'scramdb', dimensions: [{ name: 'region', type: 'string' }],
      measures: [], hierarchies: {} } as ModelInfo
    const formulas = [{ name: 'line', label: 'Line total', expr: 'amount * qty', per_row: true }, { name: 'per_sale', expr: 'revenue / transactions' }]
    const m = withRowFields(model, formulas, { line: 'number' })
    expect(m.dimensions).toEqual([{ name: 'region', type: 'string' },
      { name: 'line', label: 'Line total', type: 'number', formula: 'amount * qty', own: true }])
    expect(withRowFields(model, [], {})).toBe(model)
  })

  it('knows calculations on results', () => {
    expect(RESULT_FUNCTIONS.map(f => f.name)).toEqual(['running_sum', 'moving_avg', 'previous', 'rank', 'percent_of_total',
      'running_avg', 'next', 'difference', 'percent_change', 'dense_rank', 'along', 'within'])
    expect(usesResults('difference(revenue) / 2')).toBe(true)
    // the easy ones for everyone; dense ranks and saying along or within what, under Advanced
    const compared = (advanced: boolean) => functionGroups(RESULT_FUNCTIONS, advanced)[0].functions.map(f => f.name)
    expect(compared(false)).toEqual(['running_sum', 'moving_avg', 'previous', 'rank', 'percent_of_total',
      'running_avg', 'next', 'difference', 'percent_change'])
    expect(compared(true)).toContain('dense_rank')
    expect(compared(true)).toContain('along')
    expect(usesResults('revenue / Running_Sum (revenue)')).toBe(true)
    expect(usesResults('revenue / units')).toBe(false)
    // a field named like one isn't a call
    expect(usesResults('rank_score * 2')).toBe(false)
  })

  it('offers quick calculations on a measure, in plain words', () => {
    const calcs = quickCalcs({ ref: 'revenue', label: 'Revenue', format: 'currency' }, [{ ref: 'sum(qty)', label: 'Units' }], ['revenue'])
    expect(calcs.map(c => c.menu)).toEqual(['Share of total', 'Change from previous', '% change from previous',
      'Running total', 'Average of last 3', 'Rank', 'Divided by Units'])
    const by = (menu: string) => calcs.find(c => c.menu === menu)!.formula
    expect(by('Share of total')).toEqual({ name: 'revenue_share_of_total', label: 'Revenue, share of total', expr: 'percent_of_total(revenue)', format: 'percent' })
    // each its own name, as code would call it
    expect(calcs.map(c => c.formula.name)).toEqual(['revenue_share_of_total', 'revenue_change', 'revenue_pct_change',
      'revenue_running_total', 'revenue_avg_last_3', 'revenue_rank', 'revenue_per_units'])
    expect(by('Change from previous')).toMatchObject({ expr: 'difference(revenue)', format: 'currency' })
    expect(by('% change from previous')).toMatchObject({ expr: 'percent_change(revenue)', format: 'percent' })
    // amounts keep the measure's format
    expect(by('Running total')).toMatchObject({ expr: 'running_sum(revenue)', format: 'currency' })
    expect(by('Rank')).toMatchObject({ expr: 'rank(revenue)', format: 'number' })
    expect(by('Divided by Units')).toEqual({ name: 'revenue_per_units', label: 'Revenue per Units', expr: 'revenue / sum(qty)', format: 'number' })
    // a name already taken isn't reused
    expect(quickCalcs({ ref: 'revenue', label: 'Revenue' }, [], ['revenue_rank'])
      .find(c => c.menu === 'Rank')!.formula.name).toBe('revenue_rank_2')
  })

  it('groups the functions by what they do, the harder ones under Advanced', () => {
    const groups = functionGroups(ROW_FUNCTIONS, false)
    expect(groups.map(g => g.group)).toEqual(['Numbers', 'Logic', 'Text', 'Dates'])
    // the easy dates first; truncating, moving and counting in units under Advanced
    expect(groups.find(g => g.group === 'Dates')!.functions.map(f => f.name))
      .toEqual(['year', 'quarter', 'month', 'week', 'day', 'weekday', 'hour', 'today', 'days_between'])
    expect(functionGroups(ROW_FUNCTIONS, true).find(g => g.group === 'Dates')!.functions.map(f => f.name))
      .toContain('date_add')
    expect(groups.flatMap(g => g.functions.map(f => f.name))).not.toContain('coalesce')
    // choosing among values: in for everyone, case under Advanced
    expect(groups.find(g => g.group === 'Logic')!.functions.map(f => f.name)).toEqual(['if', 'in'])
    expect(groups.find(g => g.group === 'Text')!.functions.map(f => f.name)).toEqual(['lower', 'upper', 'concat',
      'contains', 'starts_with', 'ends_with', 'length', 'trim', 'left', 'right', 'replace'])
    expect(functionGroups(ROW_FUNCTIONS, true).find(g => g.group === 'Logic')!.functions.map(f => f.name))
      .toEqual(['if', 'coalesce', 'in', 'case'])
    const all = functionGroups(ROW_FUNCTIONS, true).flatMap(g => g.functions.map(f => f.name))
    expect(all).toContain('coalesce')
    expect(all.length).toBe(ROW_FUNCTIONS.length)
    // on totals: no text functions, but totals with a condition (not per row)
    expect(functionGroups(TOTAL_FUNCTIONS, false).map(g => g.group)).toEqual(['Numbers', 'Logic', 'Totals with a condition'])
    expect(functionGroups(TOTAL_FUNCTIONS, false).at(-1)!.functions.map(f => f.name)).toEqual(['sum_if', 'count_if', 'avg_if'])
    expect(ROW_FUNCTIONS.map(f => f.name)).not.toContain('sum_if')
  })

  it('offers quick calculations on the chart\'s measures, not on calculations on results', () => {
    const model = { measures: [{ name: 'revenue', format: 'currency' as const }, { name: 'trips', format: 'number' as const }], dimensions: [{ name: 'day' }] }
    const formulas = [{ name: 'share', label: 'Share', expr: 'percent_of_total(revenue)', format: 'percent' as const },
      { name: 'per_trip', label: 'Per trip', expr: 'revenue / trips', format: 'currency' as const }]
    const chart = { model, measures: ['revenue', 'trips', 'share', 'per_trip'], formulas, dimensions: 1, label: (r: string) => r }
    const menus = (ref: string) => calcsInChart(ref, chart).map(c => c.menu)
    // another measure to divide by, but never a calculation on results
    expect(menus('revenue')).toContain('Divided by trips')
    expect(menus('revenue')).toContain('Divided by per_trip')
    expect(menus('revenue')).not.toContain('Divided by share')
    // nothing on a calculation on results
    expect(menus('share')).toEqual([])
    // the measure's format carries over; a chart formula's too
    expect(calcsInChart('revenue', chart).find(c => c.menu === 'Running total')!.formula.format).toBe('currency')
    expect(calcsInChart('per_trip', chart).find(c => c.menu === 'Running total')!.formula.format).toBe('currency')
    // names already used by the model or the chart aren't taken again
    expect(calcsInChart('revenue', { ...chart, formulas: [...formulas, { name: 'revenue_rank', expr: 'rank(revenue)' }] })
      .find(c => c.menu === 'Rank')!.formula.name).toBe('revenue_rank_2')
    // with no dimension there's one row: nothing to compare it with, only dividing
    expect(calcsInChart('revenue', { ...chart, dimensions: 0 }).map(c => c.menu)).toEqual(['Divided by trips', 'Divided by per_trip'])
  })

  it('remembers whether Advanced is open, closed until opened, and works without storage', () => {
    const store = new Map<string, string>()
    const storage = { getItem: (k: string) => store.get(k) ?? null, setItem: (k: string, v: string) => { store.set(k, v) } }
    expect(advancedOpen(storage)).toBe(false)
    rememberAdvanced(storage, true)
    expect(advancedOpen(storage)).toBe(true)
    rememberAdvanced(storage, false)
    expect(advancedOpen(storage)).toBe(false)
    // a private window may throw: closed, and nothing breaks
    const broken = { getItem: () => { throw new Error('denied') }, setItem: () => { throw new Error('denied') } }
    expect(advancedOpen(broken)).toBe(false)
    expect(() => rememberAdvanced(broken, true)).not.toThrow()
  })

  it('says what a formula gives, in words', () => {
    expect(gives('number')).toBe('a number')
    expect(gives('string')).toBe('text')
    expect(gives('boolean')).toBe('yes or no')
    expect(gives('date')).toBe('a date')
    expect(gives('time')).toBe('a date and time')
    expect(gives('datetime')).toBe('a date and time')
    expect(gives(undefined)).toBe('')
  })

  it('offers the chart\'s dimensions to say along or within what, under Advanced', () => {
    const chips = windowChips([{ ref: 'sold_at:month', label: 'Sold at (month)' }, { ref: 'region', label: 'Region' }])
    expect(chips).toEqual([
      { label: 'along Sold at (month)', insert: ', along(sold_at)' },
      { label: 'within Sold at (month)', insert: ', within(sold_at)' },
      { label: 'along Region', insert: ', along(region)' },
      { label: 'within Region', insert: ', within(region)' },
      { label: 'within the whole chart', insert: ', within()' },
    ])
    expect(windowChips([])).toEqual([])
  })

  it('offers the functions that work where the formula is', () => {
    const names = (...a: Parameters<typeof editorFunctions>) => editorFunctions(...a).map(f => f.name)
    expect(names({ perRow: false, inChart: true, blend: false })).toContain('sum_if')
    expect(names({ perRow: false, inChart: true, blend: false })).toContain('running_sum')
    expect(names({ perRow: false, inChart: false, blend: false })).not.toContain('running_sum')
    expect(names({ perRow: true, inChart: true, blend: false })).toContain('weekday')
    expect(names({ perRow: true, inChart: true, blend: false })).not.toContain('running_sum')
    // on a blend, totals are added up per model before the join: no sum_if after it
    expect(names({ perRow: false, inChart: true, blend: true })).not.toContain('sum_if')
    expect(names({ perRow: false, inChart: true, blend: true })).toContain('running_sum')
    expect(names({ perRow: true, inChart: true, blend: true })).toContain('contains')
  })
})
