import { useEffect, useState } from 'react'
import type { CSSProperties } from 'react'
import { api } from '../api'
import type { OrgThemesResponse } from '../api'
import { useAuth } from '../auth'
import { contrast, onColor, slug } from '../colors'
import { ThemeSwatch, useTheme } from '../theme'
import type { Theme } from '../theme'

type ColorKey = keyof Theme['colors']
const COLORS: [ColorKey, string][] = [
  ['bg', 'Page background'], ['surface', 'Cards'], ['surface_2', 'Hover and stripes'], ['border', 'Borders'],
  ['text', 'Text'], ['text_muted', 'Secondary text'], ['accent', 'Accent (buttons, links)'], ['accent_soft', 'Accent, soft'],
  ['good', 'Good (up)'], ['bad', 'Bad (down)'],
]

/** Org admins: the organisation's own themes, made visually or from CSS design tokens, its
 *  default light and dark theme, and CSS export for the company's own site. */
export default function ThemesPage() {
  const { auth } = useAuth()
  const { reload } = useTheme()
  const [data, setData] = useState<OrgThemesResponse | null>(null)
  const [editing, setEditing] = useState<{ theme: Theme; isNew: boolean } | null>(null)
  const [msg, setMsg] = useState<{ kind: 'ok' | 'error'; text: string } | null>(null)
  const load = () => api.orgThemes().then(setData, e => setMsg({ kind: 'error', text: e.message }))
  useEffect(() => { load() }, [])
  if (!data) return <div className="page narrow">{msg ? <div className="error">{msg.text}</div> : <div className="empty">Loading…</div>}</div>

  const all = [...data.themes, ...data.available]
  const save = async (doc: { themes: Theme[]; default_light?: string | null; default_dark?: string | null }, done: string) => {
    setMsg(null)
    try {
      await api.saveOrgThemes(doc, data.version)
      setMsg({ kind: 'ok', text: done }); setEditing(null); await load(); reload()
    } catch (e) { setMsg({ kind: 'error', text: (e as Error).message }) }
  }
  const doc = { themes: data.themes, default_light: data.default_light ?? null, default_dark: data.default_dark ?? null }
  const download = async (t: Theme) => {
    try {
      const css = await api.themeCss(t.id)
      const a = Object.assign(document.createElement('a'), { href: URL.createObjectURL(new Blob([css], { type: 'text/css' })), download: `magnum-bi-${t.id}.css` })
      a.click(); URL.revokeObjectURL(a.href)
    } catch (e) { setMsg({ kind: 'error', text: (e as Error).message }) }
  }
  const startFrom = (base: Theme) => setEditing({ isNew: true, theme: { ...base, id: '', label: '', builtin: false } })
  const remove = (t: Theme) => {
    if (!confirm(`Delete the theme "${t.label}"?`)) return
    save({ themes: data.themes.filter(x => x.id !== t.id),
      default_light: doc.default_light === t.id ? null : doc.default_light,
      default_dark: doc.default_dark === t.id ? null : doc.default_dark }, 'Deleted.')
  }

  if (editing) {
    return <ThemeEditor initial={editing.theme} isNew={editing.isNew} taken={all.map(t => t.id)} onCancel={() => setEditing(null)}
      onSave={t => save({ ...doc, themes: editing.isNew ? [...data.themes, t] : data.themes.map(x => (x.id === t.id ? t : x)) }, 'Saved.')} />
  }
  return (
    <div className="page narrow">
      <h1>Themes <span className="muted">· {auth?.org?.name ?? 'platform workspace'}</span></h1>
      <p className="muted">Magnum BI in your company's colours. Make a theme here, or import your design team's tokens as CSS; export
        any theme as CSS design tokens (<code>--mbi-…</code>) and helper classes for your own site. Everyone here sees the
        organisation's default; anyone can still pick another theme for themselves.</p>
      {msg && <div className={msg.kind}>{msg.text}</div>}

      <h2 className="section">Default for {auth?.org?.name ?? 'this workspace'}</h2>
      <div className="card person row wrap">
        {(['light', 'dark'] as const).map(mode => {
          const key = mode === 'light' ? 'default_light' : 'default_dark'
          return (
            <label key={mode} className="small">{mode === 'light' ? 'Light' : 'Dark'}{' '}
              <select value={doc[key] ?? ''} onChange={e => save({ ...doc, [key]: e.target.value || null }, 'Default saved.')}>
                <option value="">Platform default</option>
                {all.filter(t => t.mode === mode).map(t => <option key={t.id} value={t.id}>{t.label}{t.builtin ? '' : ' (ours)'}</option>)}
              </select>
            </label>
          )
        })}
      </div>

      <h2 className="section">Our themes</h2>
      {data.themes.length === 0 && <p className="muted small">None yet: start from one of the themes below, or import CSS.</p>}
      <div className="theme-list">
        {data.themes.map(t => (
          <div key={t.id} className="card person theme-row">
            <ThemeSwatch theme={t} /> <b>{t.label}</b> <span className="muted small mono">{t.id} · {t.mode}</span>
            <span className="grow" />
            <button className="link" onClick={() => setEditing({ theme: t, isNew: false })}>Edit</button>
            <button className="link" onClick={() => download(t)}>Export CSS</button>
            <button className="link danger" onClick={() => remove(t)}>Delete</button>
          </div>
        ))}
      </div>

      <h2 className="section" data-tour="theme-start">Start a new theme</h2>
      <div className="theme-list">
        {data.available.map(t => (
          <div key={t.id} className="card person theme-row">
            <ThemeSwatch theme={t} /> <span>{t.label}</span> <span className="muted small">{t.mode}</span>
            <span className="grow" />
            <button className="link" onClick={() => startFrom(t)}>Start from it</button>
            <button className="link" onClick={() => download(t)}>Export CSS</button>
          </div>
        ))}
      </div>
      <ImportCss base={doc.default_light ?? data.available[0]?.id} onImported={t => setEditing({ isNew: true, theme: { ...t, id: '', label: '' } })} />
    </div>
  )
}

/** Paste a design team's CSS tokens (--mbi-…): the editor opens with them. */
function ImportCss({ base, onImported }: { base?: string; onImported: (t: Theme) => void }) {
  const [css, setCss] = useState('')
  const [err, setErr] = useState<string | null>(null)
  const go = async () => {
    setErr(null)
    try { onImported(await api.importThemeCss(css, base)) } catch (e) { setErr((e as Error).message) }
  }
  return (
    <details className="card person import-css">
      <summary><b>Import CSS tokens</b> <span className="muted small">from your design team (as exported here: --mbi-accent, --mbi-chart-1, …)</span></summary>
      <textarea rows={8} className="mono" value={css} onChange={e => setCss(e.target.value)} placeholder={':root {\n  --mbi-accent: #0055a5;\n  --mbi-chart-1: #0055a5;\n  …\n}'} />
      {err && <div className="error">{err}</div>}
      <div className="row"><button className="primary" disabled={!css.trim()} onClick={go}>Open in the editor</button></div>
    </details>
  )
}

function ThemeEditor({ initial, isNew, taken, onSave, onCancel }: {
  initial: Theme; isNew: boolean; taken: string[]; onSave: (t: Theme) => void; onCancel: () => void
}) {
  const [t, setT] = useState<Theme>(initial)
  const set = (patch: Partial<Theme>) => setT({ ...t, ...patch })
  const setColor = (k: ColorKey, v: string) => setT({ ...t, colors: { ...t.colors, [k]: v.toLowerCase() } })
  const id = isNew ? (t.id || slug(t.label)) : t.id
  const idTaken = isNew && taken.includes(id)
  const c = t.colors
  const checks: [string, number, number][] = [
    ['Text on the page', contrast(c.text, c.bg), 4.5],
    ['Text on cards', contrast(c.text, c.surface), 4.5],
    ['Secondary text on cards', contrast(c.text_muted, c.surface), 4.5],
    ['Links on cards', contrast(c.accent, c.surface), 4.5],
    ['Button text on the accent', contrast(onColor(c.accent), c.accent), 4.5],
    ...t.palette.map((p, i): [string, number, number] => [`Chart colour ${i + 1} on cards`, contrast(p, c.surface), 3]),
  ]
  const weak = checks.filter(([, r, min]) => r < min)
  const preview = {
    '--bg': c.bg, '--surface': c.surface, '--surface-2': c.surface_2, '--border': c.border, '--text': c.text,
    '--text-muted': c.text_muted, '--accent': c.accent, '--accent-soft': c.accent_soft, '--on-accent': onColor(c.accent),
    '--good': c.good, '--danger': c.bad, '--radius': `${t.radius}px`, '--font': t.font,
  } as CSSProperties

  return (
    <div className="page theme-editor">
      <h1>{isNew ? 'New theme' : `Edit ${initial.label}`}</h1>
      <div className="theme-editor-grid">
        <div className="card org-card">
          <label className="small">Name<input value={t.label} onChange={e => set({ label: e.target.value })} placeholder="ACME" /></label>
          {isNew && <label className="small">Id <span className="muted">(dashboards can pin it; permanent)</span>
            <input value={id} onChange={e => set({ id: slug(e.target.value) })} /></label>}
          {idTaken && <div className="error small">The id {id} is taken.</div>}
          <label className="small">Mode
            <select value={t.mode} onChange={e => set({ mode: e.target.value as Theme['mode'] })}>
              <option value="light">Light</option><option value="dark">Dark</option>
            </select>
          </label>
          <div className="color-grid">
            {COLORS.map(([k, label]) => (
              <label key={k} className="small color-field">
                <input type="color" value={c[k]} onChange={e => setColor(k, e.target.value)} />
                <input className="mono" value={c[k]} onChange={e => setColor(k, e.target.value)} maxLength={7} />
                <span>{label}</span>
              </label>
            ))}
          </div>
          <div className="small"><b>Chart colours</b> <span className="muted">(in order; 3 to 10)</span></div>
          <div className="row wrap">
            {t.palette.map((p, i) => (
              <span key={i} className="palette-slot">
                <input type="color" value={p} onChange={e => set({ palette: t.palette.map((x, j) => (j === i ? e.target.value.toLowerCase() : x)) })} />
                {t.palette.length > 3 && <button className="icon" title="Remove" onClick={() => set({ palette: t.palette.filter((_, j) => j !== i) })}>✕</button>}
              </span>
            ))}
            {t.palette.length < 10 && <button className="link" onClick={() => set({ palette: [...t.palette, t.palette[t.palette.length - 1]] })}>+ colour</button>}
          </div>
          <label className="small">Corners: {t.radius}px
            <input type="range" min={0} max={20} value={t.radius} onChange={e => set({ radius: Number(e.target.value) })} />
          </label>
          <label className="small">Font <span className="muted">(a font-family list; the font must be installed or already loaded)</span>
            <input value={t.font} onChange={e => set({ font: e.target.value })} />
          </label>
        </div>

        <div>
          <div className="theme-preview" style={preview} data-mode={t.mode}>
            <div className="tp-top"><b>Magnum<span>BI</span></b><span>Dashboards</span><span>Explore</span></div>
            <div className="tp-body">
              <div className="tp-card">
                <div className="tp-muted">Revenue</div>
                <div className="tp-kpi">€ 2.4M <span className="tp-good">+8.1%</span> <span className="tp-bad">−2.3%</span></div>
                <div className="tp-bars">{t.palette.map((p, i) => <i key={i} style={{ background: p, height: `${30 + ((i * 37) % 60)}%` }} />)}</div>
                <p className="tp-muted">Secondary text, and <a>a link</a>.</p>
                <div className="tp-buttons"><button className="primary">Save</button><button>Cancel</button><span className="tp-badge">Badge</span></div>
              </div>
            </div>
          </div>
          <div className={weak.length ? 'warn small' : 'ok small'}>
            {weak.length === 0 ? 'Every check passes (WCAG contrast).'
              : <>Hard to read for some people: {weak.map(([w, r, min]) => `${w} ${r.toFixed(1)}:1 (want ${min}:1)`).join('; ')}.</>}
          </div>
        </div>
      </div>
      <div className="row">
        <button onClick={onCancel}>Cancel</button>
        <button className="primary" disabled={!t.label.trim() || !id || idTaken} onClick={() => onSave({ ...t, id, builtin: false })}>Save theme</button>
      </div>
    </div>
  )
}
