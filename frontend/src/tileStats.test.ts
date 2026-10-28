import { describe, expect, it } from 'vitest'
import type { QueryResult } from './api'
import { tileStats } from './tileStats'

const r = (stats: QueryResult['stats'], period?: QueryResult['period']): QueryResult =>
  ({ columns: [], rows: [[1], [2], [3]], stats, sql: null, period })

describe("a tile's stats", () => {
  it('say how many rows, how long, and whether from the cache', () => {
    expect(tileStats(r({ duration_ms: 73 }))).toBe('3 rows · 73 ms')
    expect(tileStats(r({ duration_ms: 2, cache_hit: true }))).toBe('3 rows · 2 ms · cached')
    expect(tileStats(r({}))).toBe('3 rows')
  })
})
