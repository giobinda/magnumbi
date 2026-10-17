import { describe, expect, it } from 'vitest'
import { EXPIRY_HOURS, guestPath, hoursLabel, limitModels, restrictionsLabel } from './guest'

describe('guest links', () => {
  it('last from an hour to 14 days', () => {
    expect(EXPIRY_HOURS[0]).toBe(1)
    expect(Math.max(...EXPIRY_HOURS)).toBe(14 * 24)
    expect(EXPIRY_HOURS.map(hoursLabel)).toEqual(['1 hour', '1 day', '3 days', '7 days', '14 days'])
  })

  it('say what guests are limited to, with the fields\' labels', () => {
    const models = [{ name: 'sales', label: 'Sales', dimensions: [{ name: 'region', label: 'Region' }] }]
    expect(restrictionsLabel([{ model: 'sales', dimension: 'region', values: ['EMEA', 'APAC'] }], models)).toBe('Region: EMEA, APAC')
    expect(restrictionsLabel([], models)).toBe('')
  })

  it('send a guest\'s dashboard requests to the link, signed-in ones to the dashboard', () => {
    const base = '/api/guest/acme/tok'
    expect(guestPath(base, 'd', { tile: 't' })).toBe('/api/guest/acme/tok/tiles/t/query')
    expect(guestPath(null, 'd', { tile: 't' })).toBe('/api/dashboards/d/tiles/t/query')
    expect(guestPath(base, 'd', { key: 'sales.region' })).toBe('/api/guest/acme/tok/filters/values?key=sales.region')
    expect(guestPath(null, 'd', { key: 'sales.region' })).toBe('/api/dashboards/d/filters/values?key=sales.region')
    expect(guestPath(base, 'd', { model: 'sales' })).toBe('/api/guest/acme/tok/period?model=sales')
    expect(guestPath(null, 'd', { model: 'sales' })).toBe('/api/period?model=sales')
  })

  it('limit the models behind a dashboard, a blend\'s parts rather than the blend', () => {
    const m = (name: string, blend?: { base: string; joins: { model: string; on: string[] }[] }) => ({ name, dimensions: [], blend })
    const models = [m('sales'), m('taxi'), m('weather'), m('taxi_weather', { base: 'taxi', joins: [{ model: 'weather', on: [] }] })]
    expect(limitModels(['taxi_weather', 'sales'], models).map(x => x.name)).toEqual(['sales', 'taxi', 'weather'])
  })
})
