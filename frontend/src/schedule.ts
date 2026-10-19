/** Scheduled reports: their schedule in words, and recipients as typed. */
import type { ScheduleWhen } from './api'

const DAYS = ['Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday', 'Sunday']

function ordinal(n: number): string {
  const s = n % 100 >= 11 && n % 100 <= 13 ? 'th' : ({ 1: 'st', 2: 'nd', 3: 'rd' } as Record<number, string>)[n % 10] ?? 'th'
  return `${n}${s}`
}

export function describeWhen(w: ScheduleWhen): string {
  const at = `at ${w.at} (${w.timezone})`
  if (w.every === 'weekly') return `Every ${DAYS[(w.day ?? 1) - 1]} ${at}`
  if (w.every === 'monthly') return `On the ${ordinal(w.day ?? 1)} of every month ${at}`
  return `Every day ${at}`
}

/** Addresses separated by commas, semicolons, spaces or lines; lower case, once each. */
export function parseRecipients(text: string): string[] {
  return [...new Set(text.split(/[\s,;]+/).map(s => s.trim().toLowerCase()).filter(Boolean))]
}

export const WEEKDAYS = DAYS
