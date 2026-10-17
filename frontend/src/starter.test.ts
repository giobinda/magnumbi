import { describe, expect, it } from 'vitest'
import type { ModelInfo } from './api'
import { starterDashboard } from './starter'

const model = (dims: [string, string][], measures: [string, string][]): ModelInfo => ({
  name: 'trips', label: 'Taxi trips', connection: 'warehouse', hierarchies: {},
  dimensions: dims.map(([name, type]) => ({ name, label: name, type })),
  measures: measures.map(([name, type]) => ({ name, label: name, type, format: 'number' })),
} as unknown as ModelInfo)

describe('starter dashboard', () => {
  it('has numbers, a trend over time, the top categories and details', () => {
    const d = starterDashboard(model([['borough', 'string'], ['picked_up_at', 'time'], ['payment', 'string'], ['passengers', 'number']],
      [['count', 'count'], ['fare', 'sum'], ['avg_fare', 'avg']]))
    expect(d.title).toBe('Taxi trips overview')
    expect(d.tiles.map(t => [t.type, t.query.dimensions ?? [], t.query.measures])).toEqual([
      ['kpi', [], ['count']], ['kpi', [], ['fare']], ['trend', ['picked_up_at:day'], ['count']],
      ['line', ['picked_up_at:day'], ['count']], ['hbar', ['borough'], ['count']],
      ['pie', ['payment'], ['count']], ['table', ['borough', 'payment'], ['count', 'fare']],
    ])
    expect(d.filters.map(f => f.field)).toEqual(['borough', 'payment'])
    // every tile in the 12-column grid, none overlapping on a row
    for (const t of d.tiles) expect(t.layout.x + t.layout.w).toBeLessThanOrEqual(12)
    expect(new Set(d.tiles.map(t => t.id)).size).toBe(d.tiles.length)
  })

  it('makes do without dates or a second category', () => {
    const d = starterDashboard(model([['region', 'string']], [['count', 'count']]))
    expect(d.tiles.map(t => t.type)).toEqual(['kpi', 'hbar', 'table'])
    expect(d.tiles.find(t => t.type === 'hbar')!.layout.w).toBe(12)
  })
})
