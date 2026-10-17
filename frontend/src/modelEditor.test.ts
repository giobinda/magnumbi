import { describe, expect, it } from 'vitest'
import { modelFrom, rowsFor } from './modelEditor'
import type { ModelDef } from './modelEditor'

const proposal: ModelDef = {
  name: 'trips', label: 'Trips', connection: 'warehouse', table: 'trips',
  dimensions: [{ name: 'borough', label: 'Borough', type: 'string' }, { name: 'picked_up_at', label: 'Picked up at', type: 'time' }],
  measures: [{ name: 'count', label: 'Count', type: 'count' }, { name: 'fare', label: 'Fare', type: 'sum', sql: '"fare"', format: 'number' }],
}

describe('model editor', () => {
  it('has a row per column, all in, as proposed', () => {
    expect(rowsFor(proposal).map(r => [r.column, r.include, r.role, r.type])).toEqual([
      ['borough', true, 'dimension', 'string'], ['picked_up_at', true, 'dimension', 'time'], ['fare', true, 'measure', 'sum'],
    ])
  })

  it('shows a saved model\'s choices, and the columns it left out', () => {
    const saved: ModelDef = { ...proposal, dimensions: [{ name: 'borough', label: 'Pickup borough', type: 'string' }],
      measures: [proposal.measures[0], { name: 'fare', label: 'Average fare', type: 'avg', sql: '"fare"', format: 'currency' }] }
    expect(rowsFor(proposal, saved).map(r => [r.column, r.include, r.role, r.type, r.label, r.format])).toEqual([
      ['borough', true, 'dimension', 'string', 'Pickup borough', undefined],
      ['picked_up_at', false, 'dimension', 'time', 'Picked up at', undefined],
      ['fare', true, 'measure', 'avg', 'Average fare', 'currency'],
    ])
  })

  it('turns the rows back into a model: the count first, then the columns kept', () => {
    const rows = rowsFor(proposal)
    rows[1].include = false
    rows[0] = { ...rows[0], role: 'measure', type: 'count_distinct', label: 'Boroughs' }
    const m = modelFrom({ name: 'trips', label: 'Trips', connection: 'warehouse', table: 'trips' }, rows, proposal.measures[0])
    expect(m.dimensions).toEqual([])
    expect(m.measures).toEqual([
      { name: 'count', label: 'Count', type: 'count' },
      { name: 'borough', label: 'Boroughs', type: 'count_distinct', sql: 'borough', format: 'number' },
      { name: 'fare', label: 'Fare', type: 'sum', sql: 'fare', format: 'number' },
    ])
  })
})
