import { describe, expect, it } from 'vitest'
import { deleteSteps, namesToCheck, usesOf } from './deleteUses'
import type { Blend, DashboardSummary, Lineage, ModelInfo } from './api'

const model = (name: string, connection: string, blend?: { base: string; joins: { model: string; on: string[] }[] }) =>
  ({ name, label: name.toUpperCase(), connection, dimensions: [], measures: [], blend: blend ?? null }) as unknown as ModelInfo
const models = [
  model('sales', 'files'), model('stock', 'files'), model('weather', 'meteo'),
  model('both', '', { base: 'sales', joins: [{ model: 'weather', on: ['k = k'] }] }),
  model('apart', '', { base: 'weather', joins: [{ model: 'other', on: ['k = k'] }] }),
]
const blends = [{ name: 'both', label: 'Both', base: 'sales', joins: [], editable: true }] as unknown as Blend[]
const summaries: DashboardSummary[] = [{ id: 'd1', title: 'One', tiles: 5 }, { id: 'd2', title: 'Two', tiles: 2 }]
const lineage = (dashboards: Lineage['dashboards'], reports = 0) => ({ dashboards, reports }) as unknown as Lineage

describe('what uses a dataset being deleted', () => {
  it('checks the models on a connection, and the blends of those', () => {
    expect(namesToCheck({ kind: 'connection', name: 'files' }, models)).toEqual(['sales', 'stock', 'both'])
    expect(namesToCheck({ kind: 'model', name: 'weather' }, models)).toEqual(['weather', 'both', 'apart'])
  })

  it('lists models, blends and dashboards, each dashboard once with the tiles it would lose', () => {
    const lineages = {
      sales: lineage([{ id: 'd1', title: 'One', tiles: ['a', 'b'] }], 1),
      stock: lineage([{ id: 'd1', title: 'One', tiles: ['b', 'c'] }, { id: 'd2', title: 'Two', tiles: ['x', 'y'] }]),
      both: lineage([]),
    }
    const u = usesOf({ kind: 'connection', name: 'files' }, models, lineages, { models: ['sales'], blends, dashboards: summaries })
    // what can be deleted here, and what can't (the platform's)
    expect(u.models).toEqual([{ name: 'sales', label: 'SALES', deletable: true }, { name: 'stock', label: 'STOCK', deletable: false }])
    expect(u.blends).toEqual([{ name: 'both', label: 'Both', deletable: true }])
    expect(u.dashboards).toEqual([
      { id: 'd1', title: 'One', used: 3, total: 5 },
      { id: 'd2', title: 'Two', used: 2, total: 2 },
    ])
    expect(u.reports).toBe(true)
    expect(u.unknown).toEqual([])
  })

  it('a model being deleted isn\'t listed as using itself; what couldn\'t be checked is said', () => {
    const u = usesOf({ kind: 'model', name: 'sales' }, models, { sales: lineage([]), both: null }, { models: ['sales'], blends, dashboards: [] })
    expect(u.models).toEqual([])
    expect(u.blends.map(b => b.name)).toEqual(['both'])
    expect(u.unknown).toEqual(['both'])
    expect(u.reports).toBe(false)
  })

  it('deletes what was ticked, dashboards first, the dataset itself last', () => {
    const steps = deleteSteps({ kind: 'connection', name: 'files' }, new Set(['model:sales', 'dashboard:d2', 'blend:both']))
    expect(steps).toEqual([
      { kind: 'dashboard', name: 'd2' }, { kind: 'blend', name: 'both' }, { kind: 'model', name: 'sales' },
      { kind: 'connection', name: 'files' },
    ])
    // nothing ticked: the dataset alone
    expect(deleteSteps({ kind: 'model', name: 'sales' }, new Set())).toEqual([{ kind: 'model', name: 'sales' }])
  })
})
