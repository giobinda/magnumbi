import { describe, expect, it } from 'vitest'
import type { SetupStep } from './api'
import { SKIPPABLE, shownSteps } from './getStarted'

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
})
