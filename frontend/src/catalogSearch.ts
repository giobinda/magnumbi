import type { SearchResult } from './api'

/** Where a result of the top bar's search goes: a dashboard opens, the rest is shown in the
 *  catalog (its model, with the field picked out; a glossary term). */
export function resultTarget(r: SearchResult): string {
  const q = (s: string) => encodeURIComponent(s)
  switch (r.kind) {
    case 'dashboard': return `/dashboards/${q(r.name)}`
    case 'model': return `/catalog?model=${q(r.name)}`
    case 'field': return `/catalog?model=${q(r.model ?? '')}&field=${q(r.name)}`
    case 'term': return `/catalog?term=${q(r.name)}#glossary`
  }
}

/** For those who explore: a model's or a field's data, straight away. */
export function exploreFor(r: SearchResult): string | null {
  const model = r.kind === 'model' ? r.name : r.kind === 'field' ? r.model : null
  return model ? `/explore?model=${encodeURIComponent(model)}` : null
}

/** The keys that open the search: / (when not typing somewhere), Ctrl+K or Cmd+K (anywhere). */
export function isSearchShortcut(e: { key: string; ctrlKey: boolean; metaKey: boolean }, typing: boolean): boolean {
  if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'k') return true
  return e.key === '/' && !typing && !e.ctrlKey && !e.metaKey
}

/** The chat, with the search's words as its question (asked as soon as it opens). */
export function chatTarget(q: string): string {
  return `/chat?ask=${encodeURIComponent(q.trim())}`
}
