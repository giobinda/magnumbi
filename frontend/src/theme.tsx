import { createContext, useContext, useEffect, useLayoutEffect, useMemo, useState } from 'react'
import type { ReactNode } from 'react'

export interface Theme {
  id: string; label: string; mode: 'light' | 'dark'; palette: string[]; font: string; radius: number; builtin: boolean
  colors: { bg: string; surface: string; surface_2: string; border: string; text: string; text_muted: string
    accent: string; accent_soft: string; good: string; bad: string }
}

const PREF_KEY = 'magnum.theme'  // the viewer's own choice ("auto" = follow the system)
const FALLBACK: Theme = {  // until /api/themes answers (same values as aloha)
  id: 'aloha', label: 'Aloha', mode: 'light', builtin: true, radius: 12,
  font: 'Inter, ui-sans-serif, system-ui, -apple-system, "Segoe UI", sans-serif',
  palette: ['#0a8a9e', '#d62839', '#a87800', '#1f5fa8', '#c9327d', '#2b8a3e', '#d4580a', '#7a3fa0'],
  colors: { bg: '#fff6e9', surface: '#fffdf8', surface_2: '#fdeed6', border: '#efd9b8', text: '#2a1a12', text_muted: '#6e4f3a',
    accent: '#c8102e', accent_soft: '#fde3e1', good: '#13795b', bad: '#b3261e' },
}

type Ctx = {
  themes: Theme[]; theme: Theme; preference: string; setPreference: (id: string) => void
  override: string | null; setOverride: (id: string | null) => void
}
const ThemeContext = createContext<Ctx>({ themes: [], theme: FALLBACK, preference: 'auto', setPreference: () => {}, override: null, setOverride: () => {} })
export const useTheme = () => useContext(ThemeContext)

/** A page (dashboard, embed) pins a theme id while it is mounted. */
export function useThemeOverride(id: string | null | undefined) {
  const { setOverride } = useTheme()
  useEffect(() => { setOverride(id ?? null); return () => setOverride(null) }, [id, setOverride])
}

function readPref() { try { return localStorage.getItem(PREF_KEY) || 'auto' } catch { return 'auto' } }

function luminance(hex: string) {
  const c = [1, 3, 5].map(i => parseInt(hex.slice(i, i + 2), 16) / 255).map(x => (x <= 0.04045 ? x / 12.92 : ((x + 0.055) / 1.055) ** 2.4))
  return 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}
/** Black or white, whichever reads better on `hex` (button text on the accent colour). */
function onColor(hex: string) {
  const l = luminance(hex)
  return (1.05 / (l + 0.05)) >= ((l + 0.05) / 0.05) ? '#ffffff' : '#000000'
}

function apply(t: Theme) {
  const r = document.documentElement, c = t.colors
  const vars: Record<string, string> = {
    '--bg': c.bg, '--surface': c.surface, '--surface-2': c.surface_2, '--border': c.border, '--text': c.text,
    '--text-muted': c.text_muted, '--accent': c.accent, '--accent-soft': c.accent_soft, '--on-accent': onColor(c.accent),
    '--good': c.good, '--danger': c.bad, '--radius': `${t.radius}px`, '--font': t.font,
    // a sunset stripe from the palette (shown by themes that want it: see index.css)
    '--stripe': `linear-gradient(90deg, ${t.palette.slice(0, 7).join(', ')})`,
  }
  for (const [k, v] of Object.entries(vars)) r.style.setProperty(k, v)
  r.dataset.mode = t.mode
  r.dataset.theme = t.id
  r.style.colorScheme = t.mode
}

export function ThemeProvider({ children }: { children: ReactNode }) {
  const [themes, setThemes] = useState<Theme[]>([])
  const [defaults, setDefaults] = useState({ light: 'aloha', dark: 'aloha-night' })
  const [preference, setPref] = useState(readPref)
  const [override, setOverride] = useState<string | null>(null)
  const [systemDark, setSystemDark] = useState(() => matchMedia('(prefers-color-scheme: dark)').matches)

  useEffect(() => {
    fetch('/api/themes').then(r => r.json()).then(j => {
      setThemes(j.themes); setDefaults({ light: j.default_light, dark: j.default_dark })
    }).catch(() => {})
    const mq = matchMedia('(prefers-color-scheme: dark)')
    const on = (e: MediaQueryListEvent) => setSystemDark(e.matches)
    mq.addEventListener('change', on)
    return () => mq.removeEventListener('change', on)
  }, [])

  const setPreference = (id: string) => {
    setPref(id)
    try { localStorage.setItem(PREF_KEY, id) } catch { /* private mode: keep it for this visit */ }
  }

  const theme = useMemo(() => {
    const byId = (id: string | null) => (id ? themes.find(t => t.id === id) : undefined)
    const auto = byId(systemDark ? defaults.dark : defaults.light)
    return byId(override) ?? (preference === 'auto' ? auto : byId(preference) ?? auto) ?? FALLBACK
  }, [themes, override, preference, systemDark, defaults])

  useLayoutEffect(() => apply(theme), [theme])  // before charts paint, so they read the right colours

  return (
    <ThemeContext.Provider value={{ themes, theme, preference, setPreference, override, setOverride }}>
      {children}
    </ThemeContext.Provider>
  )
}

/** Small swatch strip: the theme's surface, accent and first chart colours. */
export function ThemeSwatch({ theme }: { theme: Theme }) {
  return (
    <span className="swatch" style={{ background: theme.colors.surface, borderColor: theme.colors.border }}>
      {[theme.colors.accent, ...theme.palette.slice(0, 3)].map(c => <i key={c} style={{ background: c }} />)}
    </span>
  )
}
