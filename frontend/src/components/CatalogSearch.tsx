import { useEffect, useRef, useState } from 'react'
import { Link, useNavigate } from 'react-router-dom'
import { api } from '../api'
import type { SearchResult } from '../api'
import { useCan } from '../auth'
import { chatTarget, exploreFor, isSearchShortcut, resultTarget } from '../catalogSearch'
import { shortLabels } from '../fieldGroups'
import { useModels } from '../hooks'

const KINDS: Record<SearchResult['kind'], string> = { model: 'Model', field: 'Field', term: 'Term', dashboard: 'Dashboard' }

/** The top bar's search: what the data catalog says about a word (a field, a model, a term, a
 *  dashboard), from any page. / or Ctrl+K to search; the answer is right in the list, and picking
 *  one goes to it. Those who explore also get straight to the data. */
export default function CatalogSearch() {
  const [q, setQ] = useState('')
  const [results, setResults] = useState<SearchResult[] | null>(null)
  const [open, setOpen] = useState(false)
  const [at, setAt] = useState(0)
  const input = useRef<HTMLInputElement>(null)
  const box = useRef<HTMLDivElement>(null)
  const navigate = useNavigate()
  const can = useCan()
  const { models } = useModels()
  // the chat, when it's on: the first row asks it the question as typed
  const [chat, setChat] = useState(false)
  useEffect(() => { api.chat().then(c => setChat(c.enabled), () => setChat(false)) }, [])
  const modelLabel = (n: string | null) => (n ? models.find(m => m.name === n)?.label ?? n : null)

  useEffect(() => {
    if (!q.trim()) return
    const ctl = new AbortController()
    const t = setTimeout(() => {
      api.searchCatalog(q, ctl.signal).then(r => { setResults(r.results.slice(0, 8)); setAt(0) }, () => {})
    }, 200)
    return () => { clearTimeout(t); ctl.abort() }
  }, [q])
  // / or Ctrl+K from anywhere
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const el = e.target as HTMLElement
      const typing = el.isContentEditable || ['INPUT', 'TEXTAREA', 'SELECT'].includes(el.tagName)
      if (isSearchShortcut(e, typing)) { e.preventDefault(); input.current?.focus(); setOpen(true) }
    }
    const away = (e: MouseEvent) => { if (!box.current?.contains(e.target as Node)) setOpen(false) }
    window.addEventListener('keydown', onKey)
    document.addEventListener('mousedown', away)
    return () => { window.removeEventListener('keydown', onKey); document.removeEventListener('mousedown', away) }
  }, [])

  const shown = q.trim() && open ? (results ?? []) : null
  // the rows: "Chat about …" first (chat on), then what the catalog found
  const asking = chat && Boolean(shown)
  const rows = (asking ? 1 : 0) + (shown?.length ?? 0)
  const leave = (to: string) => { setOpen(false); setQ(''); setResults(null); input.current?.blur(); navigate(to) }
  const pickRow = (i: number) => {
    if (asking && i === 0) return leave(chatTarget(q))
    const r = shown?.[i - (asking ? 1 : 0)]
    if (r) leave(resultTarget(r))
  }
  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Escape') { setOpen(false); input.current?.blur() }
    if (!rows) return
    if (e.key === 'ArrowDown') { e.preventDefault(); setAt((at + 1) % rows) }
    if (e.key === 'ArrowUp') { e.preventDefault(); setAt((at - 1 + rows) % rows) }
    if (e.key === 'Enter') { e.preventDefault(); pickRow(at) }
  }
  return (
    <div className="catalog-find" ref={box} data-tour="search">
      <input ref={input} type="search" value={q} placeholder="Search the catalog…  /" aria-label="Search the data catalog"
        role="combobox" aria-expanded={Boolean(shown?.length)} aria-controls="catalog-find-results"
        onChange={e => { setQ(e.target.value); setOpen(true) }} onFocus={() => setOpen(true)} onKeyDown={onKeyDown} />
      {shown && (
        <div className="catalog-find-results" id="catalog-find-results" role="listbox">
          {asking && (
            <div role="option" aria-selected={at === 0} className={`catalog-find-item ask${at === 0 ? ' at' : ''}`}
              onMouseEnter={() => setAt(0)} onMouseDown={e => { e.preventDefault(); pickRow(0) }}>
              <span className="tp-badge">Chat</span> Chat about <b>“{q.trim()}”</b>
              <div className="muted small">The answer, with its chart, from your data</div>
            </div>
          )}
          {shown.length === 0 && <p className="muted small">Nothing in the catalog for “{q.trim()}”.</p>}
          {shown.map((r, n) => {
            const i = n + (asking ? 1 : 0)
            const explore = can('explore') ? exploreFor(r) : null
            return (
              <div key={`${r.kind}:${r.model}:${r.name}`} role="option" aria-selected={i === at}
                className={`catalog-find-item${i === at ? ' at' : ''}`} onMouseEnter={() => setAt(i)}
                onMouseDown={e => { e.preventDefault(); pickRow(i) }}>
                <div className="row-between">
                  <span><span className="tp-badge">{KINDS[r.kind]}</span> <b>{shortLabels([r.label])[0]}</b>
                    {r.model && <span className="muted small"> in {modelLabel(r.model)}</span>}</span>
                  {explore && <Link className="small" to={explore} onMouseDown={e => e.stopPropagation()}
                    onClick={() => { setOpen(false); setQ('') }}>Explore ›</Link>}
                </div>
                {r.text && <div className="muted small catalog-find-text">{r.text}</div>}
              </div>
            )
          })}
          <Link className="small catalog-find-all" to="/catalog" onClick={() => { setOpen(false); setQ('') }}>The whole catalog ›</Link>
        </div>
      )}
    </div>
  )
}
