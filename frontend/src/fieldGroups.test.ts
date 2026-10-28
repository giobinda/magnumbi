import { describe, expect, it } from 'vitest'
import { fieldGroups } from './fieldGroups'

const f = (name: string, label?: string) => ({ name, label })

describe('the field list', () => {
  it("groups a blend's fields under their model, each by its own name", () => {
    const g = fieldGroups([
      f('taxi.pickup_time', 'NYC taxi trips · Pickup time'),
      f('taxi.borough', 'NYC taxi trips · Pickup borough'),
      f('meteo.time', 'NYC weather (hourly) · Time'),
    ], true)
    expect(g.map(x => [x.group, x.items.map(i => i.label)])).toEqual([
      ['NYC taxi trips', ['Pickup time', 'Pickup borough']],
      ['NYC weather (hourly)', ['Time']],
    ])
  })

  it("keeps a model's own fields in one list, with no heading", () => {
    const g = fieldGroups([f('region', 'Region'), f('store_id')], false)
    expect(g).toEqual([{ group: null, items: [{ field: f('region', 'Region'), label: 'Region' }, { field: f('store_id'), label: 'Store id' }] }])
  })

  it('leaves a blend field without a model in its label in a list of its own', () => {
    const g = fieldGroups([f('a.x', 'Trips · Day'), f('ratio', 'Ratio')], true)
    expect(g.map(x => x.group)).toEqual(['Trips', null])
  })
})
