import { createContext, useCallback, useContext, useEffect, useLayoutEffect, useMemo, useState } from 'react'
import type { ReactNode } from 'react'
import { onColor } from './colors'
import { getOrg } from './api'

export interface Theme {
  id: string; label: string; mode: 'light' | 'dark'; palette: string[]; font: string; radius: number; builtin: boolean
  colors: { bg: string; surface: string; surface_2: string; border: string; text: string; text_muted: string
    accent: string; accent_soft: string; good: string; bad: string }
}

const PREF_KEY = 'magnum.theme'  // the viewer's own choice ("auto" = follow the system)
const FALLBACK: Theme = {  // until /api/themes answers (same values as magnum-light, the default)
  id: 'magnum-light', label: 'Magnum light', mode: 'light', builtin: true, radius: 10,
  font: 'Inter, ui-sans-serif, system-ui, -apple-system, "Segoe UI", sans-serif',
  palette: ['#2a78d6', '#eb6834', '#1aa674', '#c48500', '#e56a98', '#008300', '#4a3aa7', '#e34948'],
  colors: { bg: '#f6f7f9', surface: '#ffffff', surface_2: '#f1f3f6', border: '#e3e6eb', text: '#1d2330', text_muted: '#5c6676',
    accent: '#4f6bed', accent_soft: '#e9edff', good: '#006300', bad: '#c4314b' },
}

type Ctx = {
  themes: Theme[]; theme: Theme; preference: string; setPreference: (id: string) => void
  override: string | null; setOverride: (id: string | null) => void
  /** fetch the theme list again (after sign-in, an organisation switch, or saving themes) */
  reload: () => void
  /** a theme that came with a page (a public link's organisation theme): usable by its id */
  adopt: (t: Theme) => void
}
const ThemeContext = createContext<Ctx>({ themes: [], theme: FALLBACK, preference: 'auto', setPreference: () => {}, override: null, setOverride: () => {}, reload: () => {}, adopt: () => {} })
export const useTheme = () => useContext(ThemeContext)

/** A page (dashboard, embed) pins a theme id while it is mounted. */
export function useThemeOverride(id: string | null | undefined) {
  const { setOverride } = useTheme()
  useEffect(() => { setOverride(id ?? null); return () => setOverride(null) }, [id, setOverride])
}

function readPref() { try { return localStorage.getItem(PREF_KEY) || 'auto' } catch { return 'auto' } }

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
  const [defaults, setDefaults] = useState({ light: 'magnum-light', dark: 'magnum-dark' })
  const [preference, setPref] = useState(readPref)
  const [override, setOverride] = useState<string | null>(null)
  const [adopted, setAdopted] = useState<Theme[]>([])  // apart from the fetched list, which a reload replaces
  const adopt = useCallback((t: Theme) => setAdopted(a => [...a.filter(x => x.id !== t.id), t]), [])
  const [systemDark, setSystemDark] = useState(() => matchMedia('(prefers-color-scheme: dark)').matches)

  // the project's themes, plus the organisation's own once signed in (reload after sign-in or
  // switching organisation)
  const reload = useCallback(() => {
    const org = getOrg()
    fetch('/api/themes', { headers: org ? { 'x-magnum-org': org } : {} }).then(r => r.json()).then(j => {
      setThemes(j.themes); setDefaults({ light: j.default_light, dark: j.default_dark })
    }).catch(() => {})
  }, [])
  useEffect(() => {
    reload()
    const mq = matchMedia('(prefers-color-scheme: dark)')
    const on = (e: MediaQueryListEvent) => setSystemDark(e.matches)
    mq.addEventListener('change', on)
    return () => mq.removeEventListener('change', on)
  }, [reload])

  const setPreference = (id: string) => {
    setPref(id)
    try { localStorage.setItem(PREF_KEY, id) } catch { /* private mode: keep it for this visit */ }
  }

  const theme = useMemo(() => {
    const byId = (id: string | null) => (id ? adopted.find(t => t.id === id) ?? themes.find(t => t.id === id) : undefined)
    const auto = byId(systemDark ? defaults.dark : defaults.light)
    return byId(override) ?? (preference === 'auto' ? auto : byId(preference) ?? auto) ?? FALLBACK
  }, [themes, adopted, override, preference, systemDark, defaults])

  useLayoutEffect(() => apply(theme), [theme])  // before charts paint, so they read the right colours

  return (
    <ThemeContext.Provider value={{ themes, theme, preference, setPreference, override, setOverride, reload, adopt }}>
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
