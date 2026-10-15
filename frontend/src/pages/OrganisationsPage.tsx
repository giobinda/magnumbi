import { useEffect, useState } from 'react'
import { api, setOrg } from '../api'
import type { Organisation, Platform, PlatformResponse, RowRule } from '../api'
import type { FormEvent } from 'react'
import { useAuth } from '../auth'
import Restrictions from '../components/Restrictions'
import { useModels } from '../hooks'

type Row = Organisation & { id: string; isNew: boolean }
const splitList = (s: string) => s.split(/[\s,]+/).map(x => x.trim()).filter(Boolean)

/** System owners: who the system owners are, and every organisation's definition (name, email
 *  domains, available models, organisation-wide filters). Members are the Org admins' business. */
export default function OrganisationsPage() {
  const { auth, refresh } = useAuth()
  const { models } = useModels()
  const [data, setData] = useState<PlatformResponse | null>(null)
  const [owners, setOwners] = useState('')
  const [rows, setRows] = useState<Row[]>([])
  const [msg, setMsg] = useState<{ kind: 'error' | 'ok'; text: string } | null>(null)
  const [busy, setBusy] = useState(false)

  const adopt = (r: PlatformResponse) => {
    setData(r)
    setOwners(r.platform.system_owners.join(', '))
    setRows(Object.entries(r.platform.organisations).map(([id, o]) => ({ ...o, id, isNew: false })))
  }
  useEffect(() => { api.platform().then(adopt, e => setMsg({ kind: 'error', text: e.message })) }, [])

  if (auth?.org) {
    return (
      <div className="page narrow">
        <div className="warn">You're working inside {auth.org.name}. Organisations are managed from the platform workspace.</div>
        <button className="primary" onClick={() => { setOrg(null); location.assign('/organisations') }}>Go to the platform workspace</button>
      </div>
    )
  }
  if (!data) return <div className="page">{msg ? <div className="error">{msg.text}</div> : <div className="empty">Loading…</div>}</div>

  const save = async () => {
    const organisations: Platform['organisations'] = {}
    for (const r of rows) {
      const id = r.id.trim().toLowerCase()
      if (!id || !r.name.trim()) { setMsg({ kind: 'error', text: 'Every organisation needs an id and a name.' }); return }
      if (organisations[id]) { setMsg({ kind: 'error', text: `Two organisations use the id ${id}.` }); return }
      if (r.filters.some((f: RowRule) => !f.model || !f.dimension)) {
        setMsg({ kind: 'error', text: `${r.name}: finish or remove the filters that have no model or dimension yet.` }); return
      }
      organisations[id] = { name: r.name.trim(), domains: r.domains, models: r.models, filters: r.filters }
    }
    setBusy(true); setMsg(null)
    try {
      adopt(await api.savePlatform({ system_owners: splitList(owners), organisations }))
      setMsg({ kind: 'ok', text: 'Saved.' })
      await refresh()
    } catch (e) { setMsg({ kind: 'error', text: (e as Error).message }) }
    setBusy(false)
  }
  const enter = (id: string) => { setOrg(id); location.assign('/members') }
  const errors = Object.values(data.errors)

  return (
    <div className="page narrow">
      <h1>Organisations</h1>
      <p className="muted">Each organisation is an independent company: its members, dashboards and data never mix with
        another's. Here you decide what each one may use. Its Org admins manage its people. Each organisation has
        its own ScramDB instance for its dashboards and query cache.</p>

      <h2 className="section">System owners</h2>
      <div className="card person">
        <input className="wide" value={owners} onChange={e => setOwners(e.target.value)} placeholder="emails, comma-separated" />
        <p className="small muted">Run the platform: every organisation, every model, no filters. System owners set their
          own password (<code>magnum-bi user set-password</code> on the server) or sign in with Google.</p>
      </div>

      <h2 className="section">Organisations</h2>
      <div className="access-rows">
        {rows.map((r, i) => {
          const set = (patch: Partial<Row>) => setRows(rows.map((x, j) => (j === i ? { ...x, ...patch } : x)))
          const all = r.models.includes('*')
          const available = all ? models : models.filter(m => r.models.includes(m.name))
          return (
            <div key={i} className="card person org-card">
              <div className="access-row">
                <input placeholder="id (e.g. acme)" value={r.id} disabled={!r.isNew} onChange={e => set({ id: e.target.value })}
                  title={r.isNew ? 'Lowercase letters, digits, - and _' : 'The id is permanent'} />
                <input placeholder="Name" value={r.name} onChange={e => set({ name: e.target.value })} />
                {!r.isNew && <span className="muted small">{data.members[r.id] ?? 0} members</span>}
                {!r.isNew && <button className="link" onClick={() => enter(r.id)}>Open</button>}
                <button className="icon" title="Remove organisation (its members lose access)"
                  onClick={() => confirm(`Remove ${r.name || r.id}? Its members lose access.`) && setRows(rows.filter((_, j) => j !== i))}>✕</button>
              </div>
              {!r.isNew && (
                <OrgDatabase org={r.id} current={data.databases[r.id]} keySet={data.secret_key}
                  onSaved={display => setData(d => (d ? { ...d, databases: { ...d.databases, [r.id]: { display, updated_at: new Date().toISOString(), updated_by: auth?.user?.email ?? null } } } : d))} />
              )}
              <label className="small">Email domains (anyone from these joins automatically)
                <input value={r.domains.join(', ')} placeholder="acme.com" onChange={e => set({ domains: splitList(e.target.value) })} />
              </label>
              <div className="small org-models"><span>Models</span>
                <label className="check"><input type="checkbox" checked={all} onChange={e => set({ models: e.target.checked ? ['*'] : [] })} /> All</label>
                {!all && data.models.map(m => (
                  <label key={m.name} className="check">
                    <input type="checkbox" checked={r.models.includes(m.name)}
                      onChange={e => set({ models: e.target.checked ? [...r.models, m.name] : r.models.filter(x => x !== m.name) })} /> {m.label}
                  </label>
                ))}
              </div>
              <div className="small">Organisation filters (everyone in it, Org admins included; they can't change these)</div>
              <Restrictions rules={r.filters} models={available} onChange={filters => set({ filters })} addLabel="+ Add organisation filter" />
            </div>
          )
        })}
        <button className="link" onClick={() => setRows([...rows, { id: '', name: '', domains: [], models: [], filters: [], isNew: true }])}>
          + Add organisation</button>
      </div>

      {errors.length > 0 && <div className="warn">{errors.map(e => <div key={e}>{e}</div>)}</div>}
      {msg && <div className={msg.kind === 'error' ? 'error' : 'ok'}>{msg.text}</div>}
      <div className="row"><button className="primary" disabled={busy} onClick={save}>{busy ? 'Saving…' : 'Save'}</button></div>
    </div>
  )
}

/** Where an organisation's own ScramDB is: its dashboards and query cache live there. */
function OrgDatabase({ org, current, keySet, onSaved }: {
  org: string; current?: { display: string }; keySet: boolean; onSaved: (display: string) => void
}) {
  const [editing, setEditing] = useState(false)
  const [dsn, setDsn] = useState('')
  const [busy, setBusy] = useState(false)
  const [msg, setMsg] = useState<{ kind: 'error' | 'ok'; text: string } | null>(null)
  const save = async (e: FormEvent) => {
    e.preventDefault()
    setBusy(true); setMsg(null)
    try {
      const r = await api.setOrgDatabase(org, dsn)
      setDsn(''); setEditing(false); onSaved(r.display)
      setMsg({ kind: 'ok', text: `Connected (${r.version.split(' on ')[0]}).${r.dashboards_moved ? ` ${r.dashboards_moved} dashboard(s) moved there.` : ''}` })
    } catch (x) { setMsg({ kind: 'error', text: (x as Error).message }) }
    setBusy(false)
  }
  return (
    <div className="small org-db">
      <div className="access-row">
        <b>Database</b>
        {current ? <code className="mono">{current.display}</code>
          : <span className="warn-text">not set: its members can't open dashboards yet</span>}
        {!editing && <button className="link" disabled={!keySet} onClick={() => setEditing(true)}>{current ? 'Change…' : 'Set…'}</button>}
      </div>
      {!keySet && <div className="warn-text">Set MAGNUM_SECRET_KEY on the server first (<code>magnum-bi gen-key</code>): connection strings are stored encrypted.</div>}
      {editing && (
        <form className="access-row" onSubmit={save}>
          <input className="wide" type="password" autoComplete="off" spellCheck={false} value={dsn} onChange={e => setDsn(e.target.value)}
            placeholder="postgresql://user:password@host:5432/scramdb" />
          <button className="primary" disabled={busy || !dsn.trim()}>{busy ? 'Testing…' : 'Test & save'}</button>
          <button type="button" className="link" onClick={() => { setEditing(false); setDsn('') }}>Cancel</button>
        </form>
      )}
      {editing && <p className="muted">Magnum BI connects, prepares the database, and moves the organisation's existing dashboards
        there when it has none yet. The connection string is stored encrypted and never shown again.</p>}
      {msg && <div className={msg.kind === 'error' ? 'error' : 'ok'}>{msg.text}</div>}
    </div>
  )
}
