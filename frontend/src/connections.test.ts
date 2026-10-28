import { describe, expect, it } from 'vitest'
import { missingConnections, nameForFile, sourceOf } from './connections'

describe('connections', () => {
  it('name the connections models need that are missing, never a blend\'s (it has none)', () => {
    const models = [
      { name: 'sales', connection: 'warehouse' }, { name: 'taxi', connection: 'nyc_taxi' },
      { name: 'taxi_weather', connection: '', blend: { base: 'taxi', joins: [] } }, { name: 'weather', connection: 'meteo' },
      { name: 'more_sales', connection: 'warehouse' },
    ]
    expect(missingConnections(models, ['warehouse'])).toEqual(['nyc_taxi', 'meteo'])
  })
})

describe('connections made from files', () => {
  it('names a new one after its file', () => {
    expect(nameForFile('Sales 2026 (EU).xlsx')).toBe('sales_2026_eu')
    expect(nameForFile('2026 budget.csv')).toBe('t_2026_budget')
    expect(nameForFile('.csv')).toBe('file')
  })

  it('says where one comes from', () => {
    const c = (config: Record<string, unknown>) => ({ name: 'x', type: 'duckdb' as const, config, has_secret: false, dialect: 'duckdb', updated_at: '', updated_by: null })
    expect(sourceOf(c({ tables: {}, file: 'Sales.xlsx' }))).toEqual({ kind: 'file', file: 'Sales.xlsx' })
    expect(sourceOf(c({ tables: {}, google_sheet: '1abc' }))).toEqual({ kind: 'sheet', id: '1abc' })
    expect(sourceOf(c({ tables: { a: 'a.parquet' } }))).toBeNull()
  })
})
