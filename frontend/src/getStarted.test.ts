import { describe, expect, it } from 'vitest'
import type { SetupStep } from './api'
import { SKIPPABLE, cardShown, shownSteps } from './getStarted'

const step = (id: string, optional = false): SetupStep =>
  ({ id, title: id, detail: '', done: false, optional, link: '/', guide: '' })

describe('Get started', () => {
  const steps = [step('demo', true), step('connection'), step('theme', true)]

  it('shows every step until one is skipped', () => {
    expect(shownSteps(steps, []).map(s => s.id)).toEqual(['demo', 'connection', 'theme'])
  })

  it('leaves out the sample data once skipped', () => {
    expect(SKIPPABLE).toContain('demo')
    expect(shownSteps(steps, ['demo']).map(s => s.id)).toEqual(['connection', 'theme'])
  })

  it('never skips what is needed to get going', () => {
    expect(shownSteps(steps, ['connection']).map(s => s.id)).toEqual(['demo', 'connection', 'theme'])
  })

  it('goes once what is needed is done, unless the sample data is still open', () => {
    const done = (s: SetupStep) => ({ ...s, done: true })
    const finished = steps.map(done)
    expect(cardShown(steps, false, false)).toBe(true)
    expect(cardShown(finished, false, false)).toBe(false)
    // the demo just set up everything: its ticks and its "Open" stay in view
    expect(cardShown(finished, false, true)).toBe(true)
    expect(cardShown(steps, true, false)).toBe(false)
  })
})
