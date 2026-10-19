/** Dashboard tabs: a tile without a tab, or with one no longer listed, is on the first tab. */

export function onTab<T extends { tab?: string | null }>(tiles: T[], tabs: string[], tab: string | null): T[] {
  if (!tabs.length) return tiles
  return tiles.filter(t => (t.tab && tabs.includes(t.tab) ? t.tab : tabs[0]) === tab)
}

/** The tab to open: the one asked for (e.g. ?tab= in the address) if it exists, else the first. */
export function pickTab(tabs: string[], wanted: string | null): string | null {
  return wanted && tabs.includes(wanted) ? wanted : tabs[0] ?? null
}
