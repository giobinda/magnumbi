import { describe, expect, it } from 'vitest'
import { describeWhen, parseRecipients } from './schedule'

describe('scheduled reports', () => {
  it('say when they run, in words', () => {
    expect(describeWhen({ every: 'daily', at: '08:00', timezone: 'Europe/Rome' })).toBe('Every day at 08:00 (Europe/Rome)')
    expect(describeWhen({ every: 'weekly', day: 1, at: '09:30', timezone: 'UTC' })).toBe('Every Monday at 09:30 (UTC)')
    expect(describeWhen({ every: 'weekly', day: 7, at: '18:00', timezone: 'UTC' })).toBe('Every Sunday at 18:00 (UTC)')
    expect(describeWhen({ every: 'monthly', day: 1, at: '07:00', timezone: 'UTC' })).toBe('On the 1st of every month at 07:00 (UTC)')
    expect(describeWhen({ every: 'monthly', day: 22, at: '07:00', timezone: 'UTC' })).toBe('On the 22nd of every month at 07:00 (UTC)')
  })

  it('read recipients however they are typed', () => {
    expect(parseRecipients('ana@acme.example, Bob@Acme.example\n\ncarl@partner.example; ana@acme.example')).toEqual(
      ['ana@acme.example', 'bob@acme.example', 'carl@partner.example'])
    expect(parseRecipients('  ')).toEqual([])
  })
})
