import { describe, expect, it } from 'vitest'
import { FUNCTIONS, RESULT_FUNCTIONS, ROW_FUNCTIONS, usesResults, insertAt, nameFor, withFormula, withRowFields, withoutFormula } from './formula'
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
    expect(FUNCTIONS.map(f => f.name)).toEqual(['if', 'round', 'coalesce', 'abs', 'least', 'greatest'])
    expect(ROW_FUNCTIONS.map(f => f.name)).toEqual(['if', 'round', 'coalesce', 'abs', 'least', 'greatest', 'lower', 'upper', 'concat'])
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
    expect(RESULT_FUNCTIONS.map(f => f.name)).toEqual(['running_sum', 'moving_avg', 'previous', 'rank', 'percent_of_total'])
    expect(usesResults('revenue / Running_Sum (revenue)')).toBe(true)
    expect(usesResults('revenue / units')).toBe(false)
    // a field named like one isn't a call
    expect(usesResults('rank_score * 2')).toBe(false)
  })
})
