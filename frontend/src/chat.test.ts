import { describe, expect, it } from 'vitest'
import { attachQuestion, basedOn, chartTile, madeLabel, madeLink, suggestions } from './chat'
import type { ModelInfo, Proposal } from './api'

const sales = {
  name: 'sales', label: 'Store sales', connection: 'w', connection_type: 'scramdb', hierarchies: {},
  dimensions: [{ name: 'sold_at', label: 'Sale time', type: 'time' }, { name: 'region', label: 'Region', type: 'string' }],
  measures: [{ name: 'revenue', label: 'Revenue', type: 'sum', format: 'currency' }],
  governance: { owner: null, stewards: [], status: 'certified', note: null, standing: 'reader' },
} as unknown as ModelInfo
const old = { ...sales, name: 'old', label: 'Old sales', governance: { ...sales.governance, status: 'deprecated' } } as ModelInfo

describe('chat', () => {
  it('links to a dashboard once the person has saved it', () => {
    const p: Proposal = { id: 'p', tool: 'save_dashboard', args: { id: 'sales overview' }, summary: '', details: [], status: 'applied' }
    expect(madeLink(p)).toBe('/dashboards/sales%20overview')
    expect(madeLink({ ...p, status: 'proposed' })).toBeNull()
    expect(madeLink({ ...p, tool: 'delete_dashboard' })).toBeNull()
    expect(madeLink({ ...p, tool: 'create_report', args: { dashboard: 'd' } })).toBe('/dashboards/d')
  })

  it('links a model made from a file to Explore', () => {
    const p: Proposal = { id: 'p', tool: 'add_file_model', args: { name: 'my orders' }, summary: '', details: [], status: 'applied' }
    expect(madeLink(p)).toBe('/explore?model=my%20orders')
    expect(madeLabel(p)).toBe('Open in Explore')
    expect(madeLabel({ ...p, tool: 'save_dashboard' })).toBe('Open the dashboard')
  })

  it('asks what was typed with an attached file, or to add it', () => {
    expect(attachQuestion(' Only the 2026 sheet ', 'sales.xlsx')).toBe('Only the 2026 sheet')
    expect(attachQuestion('  ', 'sales.xlsx')).toBe('Add sales.xlsx as a data source.')
  })

  it('suggests questions from certified models and the glossary', () => {
    expect(suggestions([old, sales], [{ name: 'Net revenue' }])).toEqual([
      'How has Revenue changed over the last 3 months?',
      'Which Region has the highest Revenue?',
      'What does Net revenue mean?',
    ])
    // never a deprecated model; nothing to suggest without measures
    expect(suggestions([old], [])).toEqual([])
  })

  it('draws a query as a chart that suits it', () => {
    const t = chartTile({ model: 'sales', dimensions: ['sold_at:month'], measures: ['revenue'] }, 0)
    expect([t.type, t.id, t.query.model]).toEqual(['line', 'chat-0', 'sales'])
    expect(chartTile({ model: 'sales', measures: ['revenue'] }, 1).type).toBe('kpi')
  })

  it('says what an answer is based on', () => {
    expect(basedOn({ model: 'sales', dimensions: ['region'], measures: ['revenue', 'sum(qty)'] }, [sales])).toEqual({
      model: 'Store sales', status: 'certified', fields: ['Region', 'Revenue', 'Qty (Sum)'],
    })
    expect(basedOn({ model: 'nope', measures: ['x'] }, [sales]).model).toBe('nope')
  })
})
