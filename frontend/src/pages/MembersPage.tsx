import { ask } from '../dialogs'
import { useEffect, useState } from 'react'
import { api } from '../api'
import type { Members, MembersResponse, Permission, Role, RowRule } from '../api'
import { useAuth } from '../auth'
import Restrictions from '../components/Restrictions'
import SignInSettings from '../components/SignInSettings'
import { useModels } from '../hooks'

export const PERM_LABELS: Record<Permission, string> = {
  view: 'View', filter: 'Filter', drill: 'Drill down', download: 'Download data', share: 'Share', explore: 'Explore',
  create: 'Create', edit: 'Edit', view_sql: 'See SQL & YAML', manage_members: 'Manage members',
  manage_orgs: 'Manage organisations', manage_system: 'Manage system',
}
type Person = { who: string; role: Role; filters: RowRule[] }

/** Org admins: the organisation's people, their roles and personal filters. What the organisation
 *  itself may use (models, organisation filters, domains) is the system owner's, shown read-only.
 *  The API enforces all of this; this page only edits it. */
export default function MembersPage() {
  const { auth, refresh } = useAuth()
  const { models } = useModels()
  const [data, setData] = useState<MembersResponse | null>(null)
  const [cfg, setCfg] = useState<Members | null>(null)
  const [people, setPeople] = useState<Person[]>([])
  const [msg, setMsg] = useState<{ kind: 'error' | 'ok'; text: string } | null>(null)
  const [busy, setBusy] = useState(false)
  // one-time passwords just issued (shown once, never stored in the browser)
  const [issued, setIssued] = useState<Record<string, string>>({})

  const adopt = (r: MembersResponse) => {
    setData(r); setCfg(r.members)
    setPeople(Object.entries(r.members.users).map(([who, p]) => ({ who, role: p.role, filters: p.filters ?? [] })))
  }
  useEffect(() => { api.members().then(adopt, e => setMsg({ kind: 'error', text: e.message })) }, [])

  if (!data || !cfg) return <div className="page">{msg ? <div className="error">{msg.text}</div> : <div className="empty">Loading…</div>}</div>

  const incomplete = (rules: RowRule[]) => rules.some(r => !r.model || !r.dimension)
  const save = async () => {
    if (incomplete(cfg.default_filters) || people.some(p => incomplete(p.filters))) {
      setMsg({ kind: 'error', text: 'Finish or remove the filters that have no model or dimension yet.' })
      return
    }
    setBusy(true); setMsg(null)
    const users: Members['users'] = {}
    for (const p of people) if (p.who.trim()) users[p.who.trim().toLowerCase()] = { role: p.role, filters: p.filters }
    try {
      adopt(await api.saveMembers({ ...cfg, users }))
      setMsg({ kind: 'ok', text: 'Saved.' })
      await refresh()
    } catch (e) { setMsg({ kind: 'error', text: (e as Error).message }) }
    setBusy(false)
  }

  const roles = data.roles
  const roleSelect = (value: Role, onChange: (r: Role) => void, allowed = roles) => (
    <select value={value} onChange={e => onChange(e.target.value as Role)}>
      {allowed.map(r => <option key={r.id} value={r.id}>{r.label}</option>)}
    </select>
  )
  const label = (m: string) => models.find(x => x.name === m)?.label ?? m
  const perms = (Object.keys(PERM_LABELS) as Permission[]).filter(p => roles.some(r => r.permissions.includes(p)))
  const errors = Object.values(data.errors)
  const issue = async (who: string) => {
    if (!(await ask.confirm(`Give ${who} a one-time password? Their current password stops working and they choose a new one at their next sign-in.`, { ok: 'Give a one-time password' }))) return
    try {
      const r = await api.resetMemberPassword(who)
      setIssued(x => ({ ...x, [r.email]: r.password }))
      api.members().then(r => setData(d => (d ? { ...d, accounts: r.accounts, last_login: r.last_login } : d)), () => {})
    } catch (e) { setMsg({ kind: 'error', text: (e as Error).message }) }
  }
  const status = (who: string) => {
    const a = data.accounts[who]
    if (a?.disabled) return 'account disabled'
    if (a?.last_login) return `signed in ${new Date(a.last_login * 1000).toLocaleDateString()}`
    if (a?.password) return 'has a password, not signed in yet'
    return who ? 'invited, not signed in yet' : ''
  }

  return (
    <div className="page narrow">
      <h1>Members <span className="muted">· {data.org.name}</span></h1>
      <p className="muted">Who can use Magnum BI in {data.org.name}, as what, and on which rows. Personal filters add to
        the organisation's filters; they can never widen them.</p>

      <div className="card org-fixed">
        <h3>Set for {data.org.name} by the platform</h3>
        <p className="small"><b>Models:</b> {data.org.models.map(label).join(', ') || 'none'}</p>
        {data.org.filters.length > 0
          ? <p className="small"><b>Organisation filters</b> (apply to everyone here):{' '}
              {data.org.filters.map(r => `${label(r.model)} › ${r.dimension}: ${r.values.join(', ') || '(none)'}`).join('; ')}</p>
          : <p className="small muted">No organisation filters.</p>}
        {data.org.domains.length > 0 && <p className="small"><b>Joins automatically:</b> anyone @{data.org.domains.join(', @')}</p>}
      </div>

      <table className="data perm-table">
        <thead><tr><th />{roles.map(r => <th key={r.id}>{r.label}</th>)}</tr></thead>
        <tbody>
          {perms.map(p => (
            <tr key={p}><td>{PERM_LABELS[p]}</td>{roles.map(r => <td key={r.id} className="center">{r.permissions.includes(p) ? '✓' : ''}</td>)}</tr>
          ))}
        </tbody>
      </table>

      <h2 className="section">People</h2>
      <div className="access-rows">
        {people.map((p, i) => {
          const set = (patch: Partial<Person>) => setPeople(people.map((x, j) => (j === i ? { ...x, ...patch } : x)))
          const who = p.who.trim().toLowerCase()
          const saved = Boolean(data.members.users[who])
          return (
            <div key={i} className="card person">
              <div className="access-row">
                <input type="email" placeholder="email" value={p.who} onChange={e => set({ who: e.target.value })} />
                {roleSelect(p.role, role => set({ role }))}
                <span className="muted small" title="Sign-in status">{status(who)}</span>
                {data.password_login && saved && who !== auth?.user?.email && (
                  <button className="link small" title="Create a one-time password to hand over" onClick={() => issue(who)}>One-time password…</button>
                )}
                <button className="icon" title="Remove person" onClick={() => setPeople(people.filter((_, j) => j !== i))}>✕</button>
              </div>
              {issued[who] && (
                <div className="ok pw-once">One-time password for {who}: <code>{issued[who]}</code>
                  <span className="muted small">Shown once. Hand it over privately; they choose their own at the next sign-in.</span></div>
              )}
              <Restrictions rules={p.filters} models={models} onChange={filters => set({ filters })} addLabel="+ Add personal filter" />
            </div>
          )
        })}
        <button className="link" data-tour="add-person" onClick={() => setPeople([...people, { who: '', role: 'viewer', filters: [] }])}>+ Add person</button>
      </div>

      {data.org.domains.length > 0 && (<>
        <h2 className="section">Everyone else from @{data.org.domains.join(', @')}</h2>
        <div className="card person">
          <label className="access-default">Role {roleSelect(cfg.default_role, r => setCfg({ ...cfg, default_role: r }),
            roles.filter(r => r.id !== 'org_admin'))}</label>
          <Restrictions rules={cfg.default_filters} models={models} onChange={default_filters => setCfg({ ...cfg, default_filters })}
            addLabel="+ Add filter" />
        </div>
      </>)}

      {errors.length > 0 && <div className="warn">{errors.map(e => <div key={e}>{e}</div>)}</div>}
      {msg && <div className={msg.kind === 'error' ? 'error' : 'ok'}>{msg.text}</div>}
      <div className="row"><button className="primary" disabled={busy} onClick={save}>{busy ? 'Saving…' : 'Save'}</button></div>

      <h2 className="section" data-tour="sign-in">Sign-in</h2>
      <SignInSettings />
    </div>
  )
}

