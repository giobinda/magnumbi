import { describe, expect, it } from 'vitest'
import { blobOf, fileNameOf, hasPicture } from './download'

describe('downloads', () => {
  it('save under the name the server gives', () => {
    expect(fileNameOf('attachment; filename="sales-regions.csv"', 'data.csv')).toBe('sales-regions.csv')
    expect(fileNameOf(null, 'data.csv')).toBe('data.csv')
    expect(fileNameOf('attachment', 'data.zip')).toBe('data.zip')
  })

  it('offer a picture of drawn charts only', () => {
    for (const t of ['bar', 'line', 'pie', 'heatmap', 'gauge']) expect(hasPicture(t)).toBe(true)
    for (const t of ['kpi', 'table', 'pivot', 'trend', 'text']) expect(hasPicture(t)).toBe(false)
  })

  it('turn a chart\'s data URL into a file without fetching it (the page\'s CSP forbids that)', async () => {
    const blob = blobOf('data:image/png;base64,' + btoa('\x89PNG'))
    expect(blob.type).toBe('image/png')
    expect(new Uint8Array(await blob.arrayBuffer())).toEqual(new Uint8Array([0x89, 0x50, 0x4e, 0x47]))
  })
})
