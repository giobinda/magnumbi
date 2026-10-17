import { useEffect, useState } from 'react'
import { api } from '../api'
import type { GuestLink, ModelInfo, Role } from '../api'
import { EXPIRY_HOURS, hoursLabel, restrictionsLabel } from '../guest'

const GUEST_ROLES: { role: Role; label: string }[] = [
  { role: 'viewer', label: 'Viewer: sees it, uses its filters and period' },
  { role: 'user', label: 'User: also drills down' },
  { role: 'power_user', label: 'Power user: also sees each tile\'s SQL' },
]
const RANK: Role[] = ['viewer', 'user', 'power_user', 'org_admin', 'system_owner']

type Limit = { field: string; values: string }

/** Power users and up: guest links to a dashboard (link + password, up to 14 days). */
export default function GuestDialog({ id, models, myRole, me, isAdmin, onClose }: {
  id: string; models: ModelInfo[]; myRole: Role; me: string; isAdmin: boolean; onClose: () => void
}) {
  const [links, setLinks] = useState<GuestLink[]>([])
  const [role, setRole] = useState<Role>('viewer')
  const [hours, setHours] = useState(168)
  const [password, setPassword] = useState('')
  const [limits, setLimits] = useState<Limit[]>([])
  const [made, setMade] = useState<{ link: GuestLink; password: string } | null>(null)
  const [qr, setQr] = useState<string | null>(null)
  const [copied, setCopied] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  const load = () => api.guestLinks(id).then(r => setLinks(r.links), e => setError(e.message))
  useEffect(() => { load() }, [id])  // eslint-disable-line react-hooks/exhaustive-deps

  const fields = models.flatMap(m => m.dimensions.map(d => ({ key: `${m.name}.${d.name}`, label: models.length > 1 ? `${m.label ?? m.name} · ${d.label ?? d.name}` : (d.label ?? d.name) })))
  const roles = GUEST_ROLES.filter(r => RANK.indexOf(r.role) <= RANK.indexOf(myRole))
  const copy = (key: string, text: string) => navigator.clipboard.writeText(text).then(() => { setCopied(key); setTimeout(() => setCopied(null), 1500) })

  const create = async () => {
    setBusy(true); setError(null)
    try {
      const filters = limits.filter(l => l.field).map(l => {
        const [model, dimension] = l.field.split('.')
        return { model, dimension, values: l.values.split(',').map(v => v.trim()).filter(Boolean) }
      })
      const link = await api.createGuestLink(id, { expires_in_hours: hours, password, role, filters })
      setMade({ link, password: link.password ?? password })
      setPassword(''); setLimits([]); await load()
    } catch (e) { setError((e as Error).message) } finally { setBusy(false) }
  }
  const revoke = async (l: GuestLink) => {
    if (!confirm('Remove this guest link? It stops working at once, even for guests who already opened it.')) return
    try { await api.revokeGuestLink(l.id); if (made?.link.id === l.id) setMade(null); await load() } catch (e) { setError((e as Error).message) }
  }

  return (
    <div className="modal-bg" onMouseDown={e => { if (e.target === e.currentTarget) onClose() }}>
      <div className="modal share-dialog">
        <h2>Share with guests</h2>
        <p className="small">Anyone with the link <b>and its password</b> can use this dashboard, colleagues or people outside your
          organisation, without an account, until the link expires (at most 14 days). They see it as you would, with the role you
          give them and only the data you allow: your own restrictions always apply.</p>
        {error && <div className="error">{error}</div>}

        {made && (
          <div className="card share-made">
            <b>Send both to your guests</b> (the password isn't shown again):
            <div className="row"><input className="mono grow" readOnly value={made.link.url} onFocus={e => e.target.select()} />
              <button onClick={() => copy('url', made.link.url)}>{copied === 'url' ? 'Copied' : 'Copy link'}</button></div>
            <div className="row"><input className="mono grow" readOnly value={made.password} onFocus={e => e.target.select()} />
              <button onClick={() => copy('pw', made.password)}>{copied === 'pw' ? 'Copied' : 'Copy password'}</button></div>
            <p className="muted small">Tip: send the password another way than the link (a call, a different chat).</p>
          </div>
        )}

        {links.length > 0 && <h3 className="section">Guest links to this dashboard</h3>}
        {links.map(l => (
          <div key={l.id} className="card share-link">
            <div className="row">
              <input className="mono grow" readOnly value={l.url} onFocus={e => e.target.select()} />
              <button onClick={() => copy(l.id, l.url)}>{copied === l.id ? 'Copied' : 'Copy'}</button>
              <button onClick={() => setQr(qr === l.id ? null : l.id)}>QR code</button>
              {(l.created_by === me || isAdmin) && <button className="danger" onClick={() => revoke(l)}>Remove</button>}
            </div>
            <div className="muted small">
              As {GUEST_ROLES.find(r => r.role === l.role)?.label.split(':')[0] ?? l.role} until {new Date(l.expires_at).toLocaleString()}
              {l.filters.length > 0 && <> · only {restrictionsLabel(l.filters, models)}</>} · by {l.created_by}
            </div>
            {qr === l.id && <img className="share-qr" alt="QR code for this link" src={`data:image/svg+xml;utf8,${encodeURIComponent(l.qr_svg)}`} />}
          </div>
        ))}

        <h3 className="section">New guest link</h3>
        <div className="guest-form">
          <label className="small">Guests are
            <select value={role} onChange={e => setRole(e.target.value as Role)}>
              {roles.map(r => <option key={r.role} value={r.role}>{r.label}</option>)}
            </select>
          </label>
          <label className="small">The link works for
            <select value={hours} onChange={e => setHours(Number(e.target.value))}>
              {EXPIRY_HOURS.map(h => <option key={h} value={h}>{hoursLabel(h)}</option>)}
            </select>
          </label>
          <label className="small">Password <span className="muted">(at least 10 characters; leave it empty and Magnum BI makes one up)</span>
            <input type="text" autoComplete="off" spellCheck={false} value={password} onChange={e => setPassword(e.target.value)} />
          </label>
          <div className="small"><b>Limit what guests see</b> <span className="muted">(optional; on top of your own restrictions)</span></div>
          {limits.map((l, i) => (
            <div key={i} className="row">
              <select value={l.field} onChange={e => setLimits(ls => ls.map((x, j) => (j === i ? { ...x, field: e.target.value } : x)))}>
                <option value="">Choose a field…</option>
                {fields.map(f => <option key={f.key} value={f.key}>{f.label}</option>)}
              </select>
              <span className="small muted">is one of</span>
              <input className="grow" placeholder="EMEA, APAC" value={l.values} onChange={e => setLimits(ls => ls.map((x, j) => (j === i ? { ...x, values: e.target.value } : x)))} />
              <button className="icon" title="Remove" onClick={() => setLimits(ls => ls.filter((_, j) => j !== i))}>✕</button>
            </div>
          ))}
          <div><button className="link" onClick={() => setLimits(ls => [...ls, { field: '', values: '' }])}>+ Add a limit</button></div>
        </div>
        <div className="row">
          <button onClick={onClose}>Close</button>
          <button className="primary" disabled={busy} onClick={create}>Create guest link</button>
        </div>
      </div>
    </div>
  )
}
