import { describe, expect, it } from 'vitest'
import { dashboardInputs, ignoredOf, scopeNote, withIgnored } from './tileScope'
import type { ChartTile, Dashboard, ModelInfo } from './api'

const sales = { name: 'sales', label: 'Sales', dimensions: [{ name: 'region', label: 'Region', type: 'string' }, { name: 'category', type: 'string' }, { name: 'sold_at', label: 'Sold at', type: 'time' }], measures: [] } as unknown as ModelInfo
const tile = (over: Partial<ChartTile> = {}): ChartTile => ({ id: 't', type: 'bar', layout: { x: 0, y: 0, w: 6, h: 4 }, query: { model: 'sales', measures: ['revenue'] }, ...over }) as ChartTile
const dash = (over: Partial<Dashboard> = {}): Dashboard => ({
  title: 'd', tiles: [],
  filters: [{ model: 'sales', field: 'region' }, { model: 'sales', field: 'category', label: 'Kind' }, { model: 'weather', field: 'condition' }],
  period: { model: 'sales', field: 'sold_at', current: { last_n_days: 30 } },
  ...over,
}) as Dashboard

describe('which dashboard filters a tile follows', () => {
  it('lists the dashboard filters and period that reach the tile', () => {
    expect(dashboardInputs(dash(), tile(), sales)).toEqual([
      { key: 'sales.region', label: 'Region' }, { key: 'sales.category', label: 'Kind' }, { key: 'period', label: 'Period' }])
    // a tile with a period of its own isn't given the dashboard's
    expect(dashboardInputs(dash(), tile({ query: { model: 'sales', period: { field: 'sold_at', current: { last_n_days: 7 } } } }), sales)
      .map(i => i.key)).toEqual(['sales.region', 'sales.category'])
    // on a blend: the filters of its models
    const blend = { ...sales, name: 'b', dimensions: [{ name: 'weather.condition', label: 'Weather · Condition', type: 'string' }] } as unknown as ModelInfo
    expect(dashboardInputs(dash(), tile({ query: { model: 'b' } }), blend)).toEqual([{ key: 'weather.condition', label: 'Weather · Condition' }])
  })

  it('reads and writes what a tile ignores, keeping its other options', () => {
    expect(ignoredOf({ period: false, ignore_filters: ['sales.region'], stacked: true })).toEqual(['period', 'sales.region'])
    expect(ignoredOf(undefined)).toEqual([])
    expect(withIgnored({ stacked: true, period: false }, ['sales.region'])).toEqual({ stacked: true, ignore_filters: ['sales.region'] })
    expect(withIgnored({ ignore_filters: ['sales.region'] }, ['period'])).toEqual({ period: false })
    expect(withIgnored({ legend: 'right' }, [])).toEqual({ legend: 'right' })
  })

  it('says when a tile is filtered differently from the rest', () => {
    expect(scopeNote(dash(), tile(), sales)).toBeNull()
    expect(scopeNote(dash(), tile({ options: { ignore_filters: ['sales.region'], period: false } }), sales)).toEqual({
      short: 'Ignores Region, Period',
      detail: "Doesn't follow the dashboard's Region and Period.",
    })
    expect(scopeNote(dash(), tile({ options: { ignore_filters: ['sales.region', 'sales.category'], period: false } }), sales)!.short)
      .toBe('Ignores 3 dashboard filters')
    const own = tile({ query: { model: 'sales', filters: [{ field: 'category', op: 'equals', values: ['Shoes'] }], period: { field: 'sold_at', current: { last_n_days: 7 } } } })
    expect(scopeNote(dash(), own, sales)).toEqual({ short: 'Own filters', detail: 'Its own filters: Category. Its own period.' })
    // nothing to differ from: no badge
    expect(scopeNote(dash({ filters: [], period: null }), own, sales)).toBeNull()
  })
})
