import { describe, expect, it } from 'vitest'
import { definitions, deprecatedIn, describeUsage, fieldMatches, fieldTitle, modelOption, mayDecide, mayHandOver, parseList, sensitive } from './governance'
import type { ModelInfo } from './api'

const model = (name: string, status: string | null, note: string | null = null) => ({
  name, label: name.toUpperCase(), connection: 'w', connection_type: 'scramdb', dimensions: [], measures: [], hierarchies: {},
  governance: { owner: null, stewards: [], status, note, standing: 'reader' },
}) as unknown as ModelInfo

describe('governance', () => {
  it('shows the status next to a model in pickers', () => {
    expect(modelOption(model('sales', 'certified'))).toBe('SALES · certified')
    expect(modelOption(model('old', 'deprecated'))).toBe('OLD · deprecated')
    expect(modelOption(model('new', null))).toBe('NEW')
    expect(modelOption(model('wip', 'draft'))).toBe('WIP · draft')
  })

  it('lists the deprecated models a dashboard uses, once each', () => {
    const models = [model('sales', 'certified'), model('old', 'deprecated', 'use sales instead')]
    const tiles = [{ query: { model: 'old' } }, { query: { model: 'sales' } }, { query: { model: 'old' } }, {}]
    expect(deprecatedIn(tiles, models)).toEqual([{ name: 'old', label: 'OLD', note: 'use sales instead' }])
    expect(deprecatedIn([{ query: { model: 'gone' } }], models)).toEqual([])
  })

  it('knows who decides what', () => {
    expect([mayDecide('admin'), mayDecide('owner'), mayDecide('steward'), mayDecide('reader')]).toEqual([true, true, false, false])
    expect([mayHandOver('admin'), mayHandOver('owner')]).toEqual([true, false])
  })

  it('finds fields by their names, labels and synonyms', () => {
    const revenue = { name: 'revenue', label: 'Revenue', synonyms: ['turnover', 'Takings'] }
    expect(['rev', 'TURN', 'takings', ''].map(q => fieldMatches(revenue, q))).toEqual([true, true, true, true])
    expect(fieldMatches(revenue, 'units')).toBe(false)
    expect(fieldMatches({ name: 'region' }, 'reg')).toBe(true)
  })

  it('reads lists typed with commas, once each', () => {
    expect(parseList(' Turnover, takings ,, turnover ')).toEqual(['Turnover', 'takings'])
    expect(parseList('')).toEqual([])
  })

  it('knows which tags call for care', () => {
    expect(sensitive(['finance', 'pii'])).toEqual(['pii'])
    expect(sensitive(['confidential', 'audited', 'personal'])).toEqual(['confidential', 'personal'])
    expect(sensitive(undefined)).toEqual([])
  })

  it('says what it knows about a field in its tooltip', () => {
    expect(fieldTitle({ description: 'After discounts', synonyms: ['turnover'], tags: ['finance'], terms: ['Net revenue'] }))
      .toBe('After discounts\nAlso called: turnover\nGlossary: Net revenue\nTags: finance')
    expect(fieldTitle({ formula: 'amount * qty' })).toBe('= amount * qty')
    expect(fieldTitle({})).toBeUndefined()
  })

  it('says how much a model is used', () => {
    expect(describeUsage({ days: 30, queries: 42, people: 5, last: '2026-10-20T09:00:00Z' })).toBe('42 queries by 5 people in the last 30 days')
    expect(describeUsage({ days: 30, queries: 1, people: 1, last: null })).toBe('1 query by 1 person in the last 30 days')
    expect(describeUsage({ days: 30, queries: 0, people: 0, last: null })).toBe('Not queried in the last 30 days')
  })

  it('finds the glossary definitions of a field\'s terms', () => {
    const glossary = [
      { name: 'Net revenue', definition: 'After discounts and returns' },
      { name: 'Store', definition: 'A shop with its own till' },
    ]
    expect(definitions(['net revenue', 'Churn'], glossary)).toEqual([
      { name: 'Net revenue', definition: 'After discounts and returns' },
      { name: 'Churn', definition: null },
    ])
    expect(definitions(undefined, glossary)).toEqual([])
  })
})
