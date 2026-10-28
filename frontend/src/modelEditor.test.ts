import { describe, expect, it } from 'vitest'
import { lookupsOf, modelFrom, rowsFor, suggestMatch, withLookups, withoutLookups } from './modelEditor'
import type { FieldRow, ModelDef } from './modelEditor'

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

describe('drill paths in the model editor', () => {
  const row = (column: string, include = true): FieldRow => ({ column, include, role: 'dimension', type: 'string', label: column })
  const meta = { name: 'sales', label: 'Sales', connection: 'warehouse', table: 'sample_sales' }
  const count = { name: 'count', type: 'count' }

  it('saves them with the model', () => {
    const m = modelFrom(meta, [row('region'), row('store')], count, { location: ['region', 'store'] })
    expect(m.hierarchies).toEqual({ location: ['region', 'store'] })
  })

  it('follows the dimensions left in the model', () => {
    const rows = [row('country'), row('region'), row('store', false)]
    const m = modelFrom(meta, rows, count, { location: ['country', 'region', 'store'], shop: ['region', 'store'] })
    // store is out: it leaves the first path, and the second, one step short, goes
    expect(m.hierarchies).toEqual({ location: ['country', 'region'] })
  })
})

describe('lookups in the model editor', () => {
  it('proposes the columns that match', () => {
    expect(suggestMatch(['id', 'customer_id', 'amount'], ['id', 'country'], 'customer')).toEqual(['customer_id', 'id'])
    expect(suggestMatch(['store', 'amount'], ['store', 'city'], 'shop')).toEqual(['store', 'store'])
    expect(suggestMatch(['amount'], ['id', 'city'], 'shop')).toEqual(['', ''])
  })

  it("adds a lookup's ticked columns as fields, named apart from the model's own", () => {
    const base: ModelDef = { name: 'orders', connection: 'w', table: 'orders', dimensions: [{ name: 'id', type: 'string' }, { name: 'name', type: 'string' }], measures: [] }
    const m = withLookups(base, [{
      table: 'shop.customers', as: 'customer', on: [['customer_id', 'id']],
      fields: [
        { column: 'country', include: true, label: 'Country', type: 'string' },
        { column: 'name', include: true, label: 'Customer', type: 'string' },
        { column: 'email', include: false, label: 'Email', type: 'string' },
      ],
    }])
    expect(m.joins).toEqual([{ table: 'shop.customers', as: 'customer', on: [['customer_id', 'id']] }])
    expect(m.dimensions.slice(2)).toEqual([
      { name: 'country', label: 'Country', type: 'string', from: 'customer' },
      // the model has a "name" already: this one is the customer's
      { name: 'customer_name', label: 'Customer', type: 'string', from: 'customer', column: 'name' },
    ])
  })

  it('reads them back for editing', () => {
    const saved: ModelDef = {
      name: 'orders', connection: 'w', table: 'orders', measures: [],
      joins: [{ table: 'shop.customers', as: 'customer', on: [['customer_id', 'id']] }],
      dimensions: [{ name: 'id', type: 'string' }, { name: 'country', label: 'Country', type: 'string', from: 'customer' },
        { name: 'customer_name', label: 'Customer', type: 'string', from: 'customer', column: 'name' }],
    }
    const [l] = lookupsOf(saved)
    expect([l.table, l.as, l.on]).toEqual(['shop.customers', 'customer', [['customer_id', 'id']]])
    expect(l.fields.map(f => [f.column, f.label, f.include])).toEqual([['country', 'Country', true], ['name', 'Customer', true]])
    // the model's own fields stay the model's
    expect(withoutLookups(saved).dimensions.map(d => d.name)).toEqual(['id'])
  })
})
