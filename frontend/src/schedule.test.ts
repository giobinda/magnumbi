import { describe, expect, it } from 'vitest'
import { describeAlert, describeWhen, numberTiles, parseRecipients } from './schedule'

describe('scheduled reports', () => {
  it('say when they run, in words', () => {
    expect(describeWhen({ every: 'daily', at: '08:00', timezone: 'Europe/Rome' })).toBe('Every day at 08:00 (Europe/Rome)')
    expect(describeWhen({ every: 'weekly', day: 1, at: '09:30', timezone: 'UTC' })).toBe('Every Monday at 09:30 (UTC)')
    expect(describeWhen({ every: 'weekly', day: 7, at: '18:00', timezone: 'UTC' })).toBe('Every Sunday at 18:00 (UTC)')
    expect(describeWhen({ every: 'monthly', day: 1, at: '07:00', timezone: 'UTC' })).toBe('On the 1st of every month at 07:00 (UTC)')
    expect(describeWhen({ every: 'monthly', day: 22, at: '07:00', timezone: 'UTC' })).toBe('On the 22nd of every month at 07:00 (UTC)')
    expect(describeWhen({ every: 'hourly', at: '00:05', timezone: 'UTC' })).toBe('Every hour at :05 (UTC)')
  })

  it('read recipients however they are typed', () => {
    expect(parseRecipients('ana@acme.example, Bob@Acme.example\n\ncarl@partner.example; ana@acme.example')).toEqual(
      ['ana@acme.example', 'bob@acme.example', 'carl@partner.example'])
    expect(parseRecipients('  ')).toEqual([])
  })

  it('describe alerts, and offer the numbers they can watch', () => {
    expect(describeAlert({ tile: 'n', measure: 'revenue', op: 'below', value: 20000 })).toBe('revenue below 20000')
    expect(describeAlert({ tile: 'n', op: 'rises_by', value: 10 })).toBe('rises 10% or more')
    const tiles = [
      { id: 'n', type: 'kpi', title: 'Revenue', query: { model: 'sales', measures: ['revenue', 'units'] }, layout: { x: 0, y: 0, w: 3, h: 2 } },
      { id: 'r', type: 'bar', query: { model: 'sales', dimensions: ['region'], measures: ['revenue'] }, layout: { x: 0, y: 0, w: 3, h: 2 } },
      { id: 't', type: 'text', text: 'hi', layout: { x: 0, y: 0, w: 3, h: 2 } },
    ] as Parameters<typeof numberTiles>[0]
    expect(numberTiles(tiles).map(t => t.id)).toEqual(['n'])
  })
})
