import type { ConnectionInfo } from './api'

/** The connections this organisation's models read from but it hasn't set up (blends read
 *  through their models: they have none of their own). */
export function missingConnections(models: { connection: string; blend?: unknown }[], have: string[]): string[] {
  return [...new Set(models.filter(m => !m.blend && m.connection).map(m => m.connection))].filter(c => !have.includes(c))
}

/** A connection's name from a file's ("Sales 2026 (EU).xlsx" is sales_2026_eu), as the server names tables. */
export function nameForFile(filename: string): string {
  const stem = filename.replace(/\.[^.]*$/, '')
  const s = stem.toLowerCase().replace(/[^a-z0-9]+/g, '_').replace(/^_+|_+$/g, '')
  return s ? s.replace(/^(\d)/, 't_$1') : 'file'
}

/** Where a connection made from a file comes from: an uploaded file, or a Google Sheet (null: neither). */
export function sourceOf(c: ConnectionInfo): { kind: 'file'; file: string } | { kind: 'sheet'; id: string } | null {
  const x = c.config as { file?: string; google_sheet?: string }
  if (c.type !== 'duckdb') return null
  if (x.google_sheet) return { kind: 'sheet', id: x.google_sheet }
  if (x.file) return { kind: 'file', file: x.file }
  return null
}
