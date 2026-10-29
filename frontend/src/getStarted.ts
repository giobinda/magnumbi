import type { SetupStep } from './api'

/** The steps of "Get started" that can be skipped: the sample data (to start with one's own), the
 *  AI agent (not everyone has one). */
export const SKIPPABLE = ['demo', 'agent']

/** What "Get started" lists: every step but the skippable ones skipped. */
export function shownSteps(steps: SetupStep[], skipped: string[]): SetupStep[] {
  return steps.filter(s => !(SKIPPABLE.includes(s.id) && skipped.includes(s.id)))
}

/** Whether "Get started" is shown: until what's needed is done (or it's hidden). The sample data,
 *  while open, keeps it: setting it up does every needed step, and its ticks and "Open" stay in view.
 *  `always`: asked for (Get started, in the menu under your name), whatever is done. */
export function cardShown(steps: SetupStep[], hidden: boolean, demoOpen: boolean, always = false): boolean {
  if (always) return true
  if (hidden) return false
  return demoOpen || steps.some(s => !s.optional && !s.done)
}
