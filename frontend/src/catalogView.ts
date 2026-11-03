// How the catalog lays out what it knows: a model's fields by kind, searched by anything said of them.
import type { Dimension, Measure, ModelInfo } from './api'
import { fieldGroups } from './fieldGroups'
import { matches } from './search'

export type CatalogField = Dimension | Measure

/** A model's dimensions and measures, each section with its whole count, grouped by model on a
 *  blend; those the search doesn't find, and sections left empty, out. */
export function fieldSections(model: ModelInfo, query: string) {
  const found = (f: CatalogField) => matches(query, f.label, f.name, f.description, ...(f.synonyms ?? []), ...(f.examples ?? []), ...(f.tags ?? []))
  const section = (kind: 'dimension' | 'measure', title: string, fields: CatalogField[]) =>
    ({ kind, title, total: fields.length, groups: fieldGroups(fields.filter(found), Boolean(model.blend)) })
  return [section('dimension', 'Dimensions', model.dimensions), section('measure', 'Measures', model.measures)]
    .filter(s => s.groups.length > 0)
}

/** A blend's field label as its model and its field ("NYC taxi trips · Trips"). */
export function labelParts(label: string): [string | null, string] {
  const at = label.indexOf(' · ')
  return at > 0 ? [label.slice(0, at), label.slice(at + 3)] : [null, label]
}
