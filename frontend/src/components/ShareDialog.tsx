import { useEffect, useState } from 'react'
import { api } from '../api'
import type { ShareLink } from '../api'
import { cacheLabel, uncachedAccepted } from '../share'

/** Org admins: public links to a dashboard (anyone with the link sees it, without signing in). */
export default function ShareDialog({ id, org, tiles, onClose }: { id: string; org: string; tiles: number; onClose: () => void }) {
  const [links, setLinks] = useState<ShareLink[]>([])
  const [periods, setPeriods] = useState<number[]>([])
  const [allowed, setAllowed] = useState(true)
  const [cache, setCache] = useState(3600)
  const [risk, setRisk] = useState({ understood: false, typed: '' })
  const [qr, setQr] = useState<string | null>(null)
  const [copied, setCopied] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  const load = () => api.shareLinks(id).then(r => { setLinks(r.links); setPeriods(r.cache_periods); setAllowed(r.allowed) }, e => setError(e.message))
  useEffect(() => { load() }, [id])  // eslint-disable-line react-hooks/exhaustive-deps

  const live = cache === 0
  const create = async () => {
    setBusy(true); setError(null)
    try {
      await api.createShareLink(id, live ? { cache_seconds: 0, accept_uncached_risk: true } : { cache_seconds: cache })
      setRisk({ understood: false, typed: '' }); setCache(3600); await load()
    } catch (e) { setError((e as Error).message) } finally { setBusy(false) }
  }
  const revoke = async (l: ShareLink) => {
    if (!confirm('Remove this public link? It stops working at once, for everyone who has it.')) return
    try { await api.revokeShareLink(l.id); await load() } catch (e) { setError((e as Error).message) }
  }
  const copy = (l: ShareLink) => navigator.clipboard.writeText(l.url).then(() => { setCopied(l.id); setTimeout(() => setCopied(null), 1500) })

  return (
    <div className="modal-bg" onMouseDown={e => { if (e.target === e.currentTarget) onClose() }}>
      <div className="modal share-dialog">
        <h2>Share publicly</h2>
        <p className="small">Anyone with a public link sees this dashboard <b>without signing in</b>: as you see it, with its default
          filters and period, without filtering or drilling down. Links get forwarded, so treat one like the data itself. Removing a
          link stops it working at once.</p>
        {error && <div className="error">{error}</div>}

        {links.length > 0 && <h3 className="section">Links to this dashboard</h3>}
        {links.map(l => (
          <div key={l.id} className={l.cache_seconds === 0 ? 'card share-link live' : 'card share-link'}>
            <div className="row">
              <input className="mono grow" readOnly value={l.url} onFocus={e => e.target.select()} />
              <button onClick={() => copy(l)}>{copied === l.id ? 'Copied' : 'Copy'}</button>
              <button onClick={() => setQr(qr === l.id ? null : l.id)}>QR code</button>
              <button className="danger" onClick={() => revoke(l)}>Remove</button>
            </div>
            <div className="muted small">
              {l.cache_seconds > 0 ? <>Results kept {cacheLabel(l.cache_seconds)}: your data warehouse answers each tile at most once per {cacheLabel(l.cache_seconds)}.</>
                : <b className="danger-text">⚠ No cache: every visit runs the queries of {tiles} {tiles === 1 ? 'tile' : 'tiles'} on your data warehouse (risks accepted by {l.created_by}).</b>}
              {' '}Made by {l.created_by}, {new Date(l.created_at).toLocaleString()}.
            </div>
            {qr === l.id && <img className="share-qr" alt="QR code for this link" src={`data:image/svg+xml;utf8,${encodeURIComponent(l.qr_svg)}`} />}
          </div>
        ))}

        <h3 className="section">New public link</h3>
        {!allowed ? <p className="warn small">Public links are turned off on this Magnum BI (by its operators: <code>MAGNUM_PUBLIC_LINKS=0</code>).</p> : <>
          <label className="small">Visitors see results up to
            <select value={cache} onChange={e => { setCache(Number(e.target.value)); setRisk({ understood: false, typed: '' }) }}>
              {periods.map(p => <option key={p} value={p}>{cacheLabel(p)} old{p === 3600 ? ' (recommended)' : ''}</option>)}
              <option value={0}>No cache: live data (not recommended)</option>
            </select>
          </label>
          {!live && <p className="muted small">Magnum BI asks your data warehouse at most once per tile every {cacheLabel(cache)}, however many people
            open the link, and shows everyone that answer. A popular link can't overload your warehouse.</p>}
          {live && <UncachedWarning org={org} id={id} tiles={tiles} risk={risk} setRisk={setRisk} />}
          <div className="row">
            <button onClick={onClose}>Close</button>
            <button className={live ? 'danger-solid' : 'primary'} disabled={busy || (live && !uncachedAccepted(risk, id))} onClick={create}>
              {live ? 'Create a live link, with no cache' : 'Create public link'}
            </button>
          </div>
        </>}
        {!allowed && <div className="row"><button onClick={onClose}>Close</button></div>}
      </div>
    </div>
  )
}

/** What turning the cache off means, before anyone can do it. */
function UncachedWarning({ org, id, tiles, risk, setRisk }: {
  org: string; id: string; tiles: number
  risk: { understood: boolean; typed: string }; setRisk: (r: { understood: boolean; typed: string }) => void
}) {
  const queries = `the queries of this dashboard's ${tiles} ${tiles === 1 ? 'tile' : 'tiles'}`
  return (
    <div className="danger-panel">
      <h3>⚠ Without a cache, your data warehouse faces the public</h3>
      <p>Every time <b>anyone</b> opens this link, reloads it, or a chat app shows a preview of it, Magnum BI runs{' '}
        <b>{queries}</b> directly on your data warehouse. You can't control how many times that happens. That means:</p>
      <ul>
        <li><b>Overload (denial of service).</b> Anyone with the link, or a script, a bot or a crawler, can make your data warehouse work
          as hard as they like: reports, dashboards and applications across your company that depend on it can slow down or stop.</li>
        <li><b>Cost.</b> If your warehouse charges per query or per data scanned (BigQuery, Snowflake and others), every visit costs
          money, and a link posted in the wrong place can run up a large bill.</li>
        <li><b>Live mistakes are public.</b> Visitors see data the moment it changes, including errors, before anyone has checked it.</li>
        <li><b>Links spread.</b> They get forwarded, posted in chats and found by search engines. Removing the link is the only way to
          stop it.</li>
      </ul>
      <p>Need fresher data? A <b>5 minute</b> cache still limits your warehouse to one query per tile every 5 minutes, whatever the traffic.</p>
      <label className="check">
        <input type="checkbox" checked={risk.understood} onChange={e => setRisk({ ...risk, understood: e.target.checked })} />
        <span>I understand that every visit to this link queries our data warehouse, and I accept these risks for <b>{org}</b>.</span>
      </label>
      <label className="small"><span>Type the dashboard's id, <code>{id}</code>, to confirm</span>
        <input value={risk.typed} onChange={e => setRisk({ ...risk, typed: e.target.value })} autoComplete="off" spellCheck={false} />
      </label>
    </div>
  )
}
