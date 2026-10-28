/** The blend form (Data › Blends): what it proposes to match, and the match in words. */

export type Key = { base: string; other: string; grain: string }
type Field = { name: string; type: string; label?: string | null }

const TEMPORAL = ['date', 'time', 'datetime']
const same = (a: Field, b: Field) =>
  a.name.toLowerCase() === b.name.toLowerCase() || (a.label ?? a.name).toLowerCase() === (b.label ?? b.name).toLowerCase()

/** The fields to match, proposed when a model is added: those with the same name or label, or one
 *  name ending with the other's (not dates), and the first date or time of each, by the hour (moments) or the day (dates): exact
 *  instants rarely meet. One empty key when nothing stands out. */
export function suggestKeys(base: Field[], other: Field[]): Key[] {
  const keys: Key[] = []
  // one name ending with the other's: the same thing, said more precisely (pickup_borough, borough)
  const ends = (a: Field, b: Field) => a.name.endsWith(`_${b.name}`) || b.name.endsWith(`_${a.name}`)
  for (const a of base.filter(f => !TEMPORAL.includes(f.type))) {
    const fields = other.filter(o => !TEMPORAL.includes(o.type))
    const b = fields.find(o => same(a, o)) ?? fields.find(o => ends(a, o))
    if (b && !keys.some(k => k.other === b.name)) keys.push({ base: a.name, other: b.name, grain: '' })
  }
  const ta = base.find(f => TEMPORAL.includes(f.type)), tb = other.find(f => TEMPORAL.includes(f.type))
  if (ta && tb) keys.push({ base: ta.name, other: tb.name, grain: ta.type === 'date' || tb.type === 'date' ? 'day' : 'hour' })
  return keys.length ? keys : [{ base: '', other: '', grain: '' }]
}

/** "Each Taxi trips row gets the Weather of the same Borough and the same hour (Pickup time, Time)." */
export function matchSentence(base: string, other: string, keys: Key[],
  labelOf: { base: (field: string) => string; other: (field: string) => string }): string {
  const parts = keys.filter(k => k.base && k.other).map(k => {
    const [a, b] = [labelOf.base(k.base), labelOf.other(k.other)]
    if (k.grain) return `the same ${k.grain} (${a}, ${b})`
    return a === b ? `the same ${a}` : `${a} = ${b}`
  })
  return parts.length ? `Each ${base} row gets the ${other} of ${parts.join(' and ')}.` : ''
}

/** A blend's id from its label ("Taxi trips × weather" is taxi_trips_weather). */
export function blendName(label: string): string {
  const s = label.toLowerCase().replace(/[^a-z0-9]+/g, '_').replace(/^_+|_+$/g, '')
  return s.replace(/^(\d)/, 't_$1')
}
