import { describe, expect, it } from 'vitest'
import { blendName, matchSentence, suggestKeys } from './blendForm'

const d = (name: string, type = 'string', label?: string) => ({ name, type, label: label ?? name })

describe('the blend form', () => {
  it('proposes the fields to match: the same name or label, and the times', () => {
    const taxi = [d('pickup_time', 'time', 'Pickup time'), d('pickup_borough', 'string', 'Pickup borough'), d('vendor')]
    const weather = [d('time', 'time', 'Time'), d('borough', 'string', 'Borough'), d('condition')]
    expect(suggestKeys(taxi, weather)).toEqual([
      // the same thing, said more precisely on one side (pickup_borough, borough)
      { base: 'pickup_borough', other: 'borough', grain: '' },
      // two moments in time: the same hour (exact instants rarely meet)
      { base: 'pickup_time', other: 'time', grain: 'hour' },
    ])
  })

  it('matches dates by the day, and leaves a key to fill when nothing matches', () => {
    expect(suggestKeys([d('sold_on', 'date')], [d('day', 'date')])).toEqual([{ base: 'sold_on', other: 'day', grain: 'day' }])
    expect(suggestKeys([d('a')], [d('b')])).toEqual([{ base: '', other: '', grain: '' }])
  })

  it('says in words what it joins', () => {
    expect(matchSentence('Taxi trips', 'Weather', [
      { base: 'pickup_borough', other: 'borough', grain: '' }, { base: 'pickup_time', other: 'time', grain: 'hour' },
    ], {
      base: f => ({ pickup_borough: 'Borough', pickup_time: 'Pickup time' }[f] ?? f),
      other: f => ({ borough: 'Borough', time: 'Time' }[f] ?? f),
    }))
      .toBe('Each Taxi trips row gets the Weather of the same Borough and the same hour (Pickup time, Time).')
  })

  it('makes the id from the label', () => {
    expect(blendName('Taxi trips × weather')).toBe('taxi_trips_weather')
    expect(blendName('2026 sales')).toBe('t_2026_sales')
  })
})
