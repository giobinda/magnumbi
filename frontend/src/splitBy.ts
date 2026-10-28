/** "Split by" in the tile editor: a series per value of a dimension (trips by borough, a line each).
 *  It's the chart's second dimension (chartOption: one measure, split by the second dimension). */

export const splitOf = (dims: string[]): string => dims[1] ?? ''

/** The dimensions with `field` as the split ('' removes it); the first stays the axis. */
export function splitBy(dims: string[], field: string): string[] {
  const first = dims.slice(0, 1)
  const same = field.split(':')[0] === first[0]?.split(':')[0]
  return field && !same ? [...first, field] : first
}

/** Why a split shows nothing: a chart splits one measure (with more, a series per measure). */
export function splitNote(dims: string[], measures: string[]): string | null {
  return dims.length > 1 && measures.length > 1 ? 'Split by works with one measure: keep one to see a series per value.' : null
}
