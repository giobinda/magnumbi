import { describe, expect, it } from 'vitest'
import { fieldSections, labelParts } from './catalogView'
import type { ModelInfo } from './api'

const model = {
  name: 'sales',
  dimensions: [{ name: 'region', label: 'Region', type: 'string', description: 'Where it was sold' }, { name: 'store', type: 'string', synonyms: ['shop'] }],
  measures: [{ name: 'revenue', label: 'Revenue', type: 'sum', format: 'currency', examples: ['1200'] }],
} as unknown as ModelInfo
const blend = {
  name: 'both', blend: { base: 'taxi', joins: [] },
  dimensions: [{ name: 'taxi.zone', label: 'Taxi · Zone', type: 'string' }, { name: 'weather.sky', label: 'Weather · Sky', type: 'string' }],
  measures: [],
} as unknown as ModelInfo

describe('how the catalog lays out a model\'s fields', () => {
  it('splits dimensions from measures, each with how many', () => {
    const s = fieldSections(model, '')
    expect(s.map(x => [x.kind, x.title, x.total])).toEqual([['dimension', 'Dimensions', 2], ['measure', 'Measures', 1]])
    expect(s[0].groups[0].items.map(i => i.label)).toEqual(['Region', 'Store'])
  })

  it('finds a field by name, id, definition, or what else it\'s called, and drops empty sections', () => {
    const names = (q: string) => fieldSections(model, q).flatMap(s => s.groups.flatMap(g => g.items.map(i => i.field.name)))
    expect(names('revenue')).toEqual(['revenue'])
    expect(names('sold')).toEqual(['region'])
    expect(names('shop')).toEqual(['store'])
    expect(fieldSections(model, 'revenue').map(s => s.kind)).toEqual(['measure'])
    expect(fieldSections(model, 'nothing like it')).toEqual([])
    // the count stays the section's whole, the search says how many it found
    expect(fieldSections(model, 'shop')[0].total).toBe(2)
  })

  it('on a blend, groups fields by their model, without repeating it', () => {
    const groups = fieldSections(blend, '')[0].groups
    expect(groups.map(g => g.group)).toEqual(['Taxi', 'Weather'])
    expect(groups[0].items[0].label).toBe('Zone')
  })

  it('splits a blend\'s label into its model and its field', () => {
    expect(labelParts('NYC taxi trips · Pickup time')).toEqual(['NYC taxi trips', 'Pickup time'])
    expect(labelParts('Revenue')).toEqual([null, 'Revenue'])
  })
})
