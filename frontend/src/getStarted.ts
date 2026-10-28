import type { SetupStep } from './api'

/** The steps of "Get started" that can be skipped: the sample data, to start with one's own. */
export const SKIPPABLE = ['demo']

/** What "Get started" lists: every step but the skippable ones skipped. */
export function shownSteps(steps: SetupStep[], skipped: string[]): SetupStep[] {
  return steps.filter(s => !(SKIPPABLE.includes(s.id) && skipped.includes(s.id)))
}
