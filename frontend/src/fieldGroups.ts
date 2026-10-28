/** The field list in Explore and the tile editor: a blend's fields are labelled "<model> · <field>",
 *  so a narrow list showed only "NYC taxi trips · …". They're grouped under their model instead,
 *  each by its own name. A model's own fields stay one list. */
export function fieldGroups<T extends { name: string; label?: string | null }>(
  fields: T[], blend: boolean,
): { group: string | null; items: { field: T; label: string }[] }[] {
  const groups: { group: string | null; items: { field: T; label: string }[] }[] = []
  for (const field of fields) {
    const full = field.label || (field.name.charAt(0).toUpperCase() + field.name.slice(1)).replace(/_/g, ' ')
    const at = blend ? full.indexOf(' · ') : -1
    const [group, label] = at > 0 ? [full.slice(0, at), full.slice(at + 3)] : [null, full]
    let g = groups.find(x => x.group === group)
    if (!g) groups.push(g = { group, items: [] })
    g.items.push({ field, label })
  }
  return groups
}

/** A blend's field labels without their model ("NYC taxi trips · Trips" is Trips), for a tile's
 *  title; a field keeps its model where another would read the same. */
export function shortLabels(labels: string[]): string[] {
  const short = labels.map(l => (l.includes(' · ') ? l.slice(l.indexOf(' · ') + 3) : l))
  return short.map((s, i) => (short.filter(x => x === s).length > 1 ? labels[i] : s))
}
