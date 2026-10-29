import { describe, expect, it } from 'vitest'
import { filterChoices, filtersFor, flippable, linkChoices, newFilter, opOf, periodChoices, periodFor, tileFilters, withLink } from './dashboardFilters'
import type { Dashboard, ModelInfo } from './api'

const model = (name: string, label: string, dims: [string, string][], blend = false) =>
  ({ name, label, dimensions: dims.map(([n, type]) => ({ name: n, label: n.replace(/^\w+\./, '').replace('_', ' '), type })), measures: [], blend: blend ? { base: 'taxi', joins: [{ model: 'weather', on: [] }] } : null }) as unknown as ModelInfo
const taxi = model('taxi', 'Taxi', [['pickup_borough', 'string'], ['pickup_at', 'time'], ['dropoff_at', 'time'], ['fare', 'number']])
const weather = model('weather', 'Weather', [['condition', 'string'], ['borough', 'string'], ['observed_on', 'date'], ['observed_at', 'time']])
const both = model('taxi_weather', 'Taxi × weather', [['taxi.pickup_borough', 'string'], ['weather.condition', 'string']], true)
const dash = {
  title: 'd', tiles: [
    { id: 'a', type: 'bar', layout: { x: 0, y: 0, w: 6, h: 4 }, query: { model: 'taxi' } },
    { id: 'b', type: 'bar', layout: { x: 0, y: 0, w: 6, h: 4 }, query: { model: 'taxi_weather' } },
    { id: 'n', type: 'text', layout: { x: 0, y: 0, w: 6, h: 2 }, text: 'hi' },
  ],
  filters: [{ model: 'taxi', field: 'pickup_borough' }, { model: 'taxi', field: 'fare', op: 'not_null' }],
} as unknown as Dashboard
const models = [taxi, weather, both]

describe('a dashboard\'s filters', () => {
  it('flips between is and is not, only where a filter is one of those', () => {
    const [borough, fare] = dash.filters
    expect(opOf(borough, {})).toBe('equals')
    expect(opOf(borough, { 'taxi.pickup_borough': 'not_equals' })).toBe('not_equals')
    expect(flippable(borough)).toBe(true)
    expect(flippable(fare)).toBe(false)
    expect(opOf(fare, { 'taxi.fare': 'equals' })).toBe('not_null')
  })

  it('gives each model its filters, with the viewer\'s is or is not', () => {
    const out = filtersFor(dash, { 'taxi.pickup_borough': ['Queens'] }, { 'taxi.pickup_borough': 'not_equals' })
    expect(out.taxi).toEqual([
      { field: 'pickup_borough', op: 'not_equals', values: ['Queens'] },
      { field: 'fare', op: 'not_null', values: [] },
    ])
    // nothing picked: no filter, unless it needs no value
    expect(filtersFor(dash, {}, {}).taxi).toEqual([{ field: 'fare', op: 'not_null', values: [] }])
  })

  it('offers the fields of the dashboard\'s models to filter on, and their dates for the period', () => {
    // the models of its tiles, a blend's own models: not the blend itself; not the dates
    expect(filterChoices(dash, models).map(c => `${c.model}.${c.field}`))
      .toEqual(['taxi.pickup_borough', 'taxi.fare', 'weather.condition', 'weather.borough'])
    expect(filterChoices(dash, models)[0].label).toBe('Taxi · pickup borough')
    expect(periodChoices(dash, models).map(c => `${c.model}.${c.field}`))
      .toEqual(['taxi.pickup_at', 'taxi.dropoff_at', 'weather.observed_on', 'weather.observed_at'])
  })

  it('makes a new filter: a label only when given', () => {
    expect(newFilter('taxi', 'payment', 'not_equals', '')).toEqual({ model: 'taxi', field: 'payment', op: 'not_equals', default: [] })
    expect(newFilter('taxi', 'payment', 'equals', ' Paid by ')).toEqual({ model: 'taxi', field: 'payment', op: 'equals', default: [], label: 'Paid by' })
  })

  it('reaches other models through the field named on each', () => {
    const linked = { ...dash, filters: [{ model: 'taxi', field: 'pickup_borough', fields: { weather: 'borough' } }] } as Dashboard
    expect(filtersFor(linked, { 'taxi.pickup_borough': ['Queens'] }, {}).weather)
      .toEqual([{ field: 'borough', op: 'equals', values: ['Queens'] }])
    const period = { model: 'taxi', field: 'pickup_at', fields: { weather: 'observed_at' }, current: { last_n_days: 7 } }
    expect(periodFor(period, taxi)).toEqual({ field: 'pickup_at', current: { last_n_days: 7 } })
    expect(periodFor(period, weather)).toEqual({ field: 'observed_at', current: { last_n_days: 7 } })
    expect(periodFor(period, model('zones', 'Zones', []))).toBeNull()
    expect(periodFor(null, taxi)).toBeNull()
    // a blend of the model: its own name for the field
    expect(periodFor(period, both)).toEqual({ field: 'taxi.pickup_at', current: { last_n_days: 7 } })
    const byModel = filtersFor(linked, { 'taxi.pickup_borough': ['Queens'] }, {})
    expect(tileFilters(byModel, both)).toEqual([
      { field: 'taxi.pickup_borough', op: 'equals', values: ['Queens'] },
      { field: 'weather.borough', op: 'equals', values: ['Queens'] },
    ])
    expect(tileFilters(byModel, taxi)).toEqual([{ field: 'pickup_borough', op: 'equals', values: ['Queens'] }])
    expect(tileFilters(byModel, undefined)).toBeUndefined()
  })

  it('suggests the matching field on each other model, to change or turn off', () => {
    // the same name, a part of it, a date of the same kind
    const borough = linkChoices(dash, models, 'taxi', 'pickup_borough')
    expect(borough.map(l => [l.model, l.suggested])).toEqual([['weather', 'borough']])
    expect(borough[0].options.map(o => o.field)).toEqual(['condition', 'borough'])
    expect(linkChoices(dash, models, 'taxi', 'pickup_at')[0].suggested).toBe('observed_at')
    expect(linkChoices(dash, models, 'taxi', 'fare')[0].suggested).toBe('')
    // set, changed, turned off
    const f: { model: string; field: string; fields?: Record<string, string> } = { model: 'taxi', field: 'pickup_borough' }
    expect(withLink(f, 'weather', 'borough')).toEqual({ ...f, fields: { weather: 'borough' } })
    expect(withLink({ ...f, fields: { weather: 'borough' } }, 'weather', '')).toEqual(f)
  })
})
