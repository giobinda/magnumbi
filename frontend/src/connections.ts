/** The connections this organisation's models read from but it hasn't set up (blends read
 *  through their models: they have none of their own). */
export function missingConnections(models: { connection: string; blend?: unknown }[], have: string[]): string[] {
  return [...new Set(models.filter(m => !m.blend && m.connection).map(m => m.connection))].filter(c => !have.includes(c))
}
