import { describe, expect, it } from 'vitest'
import { deprecatedIn, modelOption, mayDecide, mayHandOver } from './governance'
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
})
