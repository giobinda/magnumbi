// Colour arithmetic for the theme editor and the theme provider (WCAG 2 definitions).

export function luminance(hex: string): number {
  const c = [1, 3, 5].map(i => parseInt(hex.slice(i, i + 2), 16) / 255).map(x => (x <= 0.04045 ? x / 12.92 : ((x + 0.055) / 1.055) ** 2.4))
  return 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/** Contrast ratio of two #rrggbb colours (1 to 21; text wants 4.5, graphics 3). */
export function contrast(a: string, b: string): number {
  const [x, y] = [luminance(a), luminance(b)]
  return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05)
}

/** Black or white, whichever reads better on `hex` (button text on the accent colour). */
export function onColor(hex: string): string {
  const l = luminance(hex)
  return (1.05 / (l + 0.05)) >= ((l + 0.05) / 0.05) ? '#ffffff' : '#000000'
}

/** A theme id from a name: lowercase letters, digits and dashes. */
export function slug(name: string): string {
  return name.normalize('NFKD').toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 40)
}
