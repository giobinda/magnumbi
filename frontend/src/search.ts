// Searching long lists: what a search finds, what counts as long, and the Picker's arithmetic.

const plain = (s: string) => s.normalize('NFD').replace(/\p{M}/gu, '').toLowerCase()

/** Whether every word of the search is in what's given, capitals and accents aside. */
export function matches(query: string, ...texts: (string | null | undefined)[]): boolean {
  const all = plain(texts.filter(Boolean).join(' '))
  return plain(query).split(/\s+/).filter(Boolean).every(w => all.includes(w))
}

/** As `matches`, and a row just added (nothing in it yet) stays in sight. */
export function matchesOrBlank(query: string, ...texts: (string | null | undefined)[]): boolean {
  return !texts.some(t => t?.trim()) || matches(query, ...texts)
}

/** Past this many, a list gets a search box. */
export const LONG_LIST = 8
export const isLong = (n: number) => n > LONG_LIST

export type Option = { value: string; label: string; hint?: string | null; group?: string; disabled?: boolean }

export const pickable = (options: Option[], query: string) =>
  options.filter(o => matches(query, o.label, o.value, o.hint, o.group))

/** The option the arrows land on from `at`: the next that can be picked, or where it was. */
export function nextActive(options: Option[], at: number, step: 1 | -1): number {
  for (let i = at + step; i >= 0 && i < options.length; i += step) {
    if (!options[i].disabled) return i
  }
  return at
}

const WIDEST_LIST = 360
const NARROWEST = 320
const GAP = 4
const MARGIN = 8

/** Where a Picker's list opens: under its button or, without room, above it; inside the window. */
export function placePopover(
  button: { left: number; top: number; bottom: number; width: number },
  window: { width: number; height: number },
): { left: number; width: number; maxHeight: number; top?: number; bottom?: number } {
  const width = Math.min(Math.max(button.width, NARROWEST), window.width - 2 * MARGIN)
  const left = Math.max(MARGIN, Math.min(button.left, window.width - width - MARGIN))
  const below = window.height - button.bottom - GAP - MARGIN
  const above = button.top - GAP - MARGIN
  return below >= WIDEST_LIST || below >= above
    ? { left, width, top: button.bottom + GAP, maxHeight: Math.min(WIDEST_LIST, below) }
    : { left, width, bottom: window.height - button.top + GAP, maxHeight: Math.min(WIDEST_LIST, above) }
}
