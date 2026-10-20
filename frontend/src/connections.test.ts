import { describe, expect, it } from 'vitest'
import { missingConnections } from './connections'

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
