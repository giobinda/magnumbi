import { describe, expect, it } from 'vitest'
import { accessOf, canBeAllowedData } from './members'

describe('a person in the members list', () => {
  it('may be allowed to add data only as a power user', () => {
    expect(canBeAllowedData('power_user')).toBe(true)
    for (const r of ['viewer', 'user', 'org_admin'] as const) expect(canBeAllowedData(r)).toBe(false)
  })

  it('saves the flag only where it means something', () => {
    expect(accessOf({ role: 'power_user', filters: [], add_data: true })).toEqual({ role: 'power_user', filters: [], add_data: true })
    expect(accessOf({ role: 'power_user', filters: [], add_data: false })).toEqual({ role: 'power_user', filters: [] })
    // ticked, then moved to another role: off, so moving back later doesn't turn it on again
    expect(accessOf({ role: 'viewer', filters: [], add_data: true })).toEqual({ role: 'viewer', filters: [] })
    expect(accessOf({ role: 'org_admin', filters: [], add_data: true })).toEqual({ role: 'org_admin', filters: [] })
  })
})
