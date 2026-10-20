import { useEffect, useState } from 'react'
import type { FormEvent } from 'react'
import { api } from '../api'
import type { DeliverySettings, MailSecurity, MailServerInfo, Schedule, WebhookKind } from '../api'
import { describeAlert, describeWhen } from '../schedule'
import { useAuth } from '../auth'
import { defaultPort, mailBody } from '../delivery'
import type { MailForm } from '../delivery'

const SECURITY: Record<MailSecurity, string> = { starttls: 'STARTTLS (port 587)', tls: 'TLS (port 465)', none: 'None (internal relay, no password)' }
const KINDS: Record<WebhookKind, string> = { slack: 'Slack', teams: 'Microsoft Teams', webhook: 'Other (JSON)' }

/** Org admins (system owners in the platform workspace): where this workspace's reports and
 *  alerts are sent from. Each organisation, and the platform workspace, has its own. */
export default function DeliveryPage() {
  const { auth } = useAuth()
  const [data, setData] = useState<DeliverySettings | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [editMail, setEditMail] = useState(false)
  const [newHook, setNewHook] = useState(false)
  const load = () => api.delivery().then(setData, e => setError(e.message))
  useEffect(() => { load() }, [])
  const where = auth?.org?.name ?? 'the platform workspace'

  return (
    <div className="page narrow">
      <h1>Email &amp; webhooks <span className="muted">· {auth?.org?.name ?? 'platform workspace'}</span></h1>
      <p className="muted">Where reports and alerts from {where} are sent from: its own mail server, and Slack, Teams or other
        webhooks. Nothing here is shared with {auth?.org ? 'other organisations or the platform' : 'organisations: each sets up its own'}.
        Passwords and webhook addresses are stored encrypted and never shown again.</p>
      {data && !data.secret_key && <div className="warn">MAGNUM_SECRET_KEY isn't set on the server: mail passwords and webhooks can't be saved.</div>}
      {error && <div className="error">{error}</div>}

      <h2 className="section">Mail server</h2>
      {data && !editMail && (data.mail
        ? <MailSummary mail={data.mail} onEdit={() => setEditMail(true)} onRemoved={load} />
        : <><p className="muted small">No mail server yet: reports can't be emailed from here.</p>
          <div className="row"><button className="primary" onClick={() => setEditMail(true)}>+ Set up a mail server</button></div></>)}
      {editMail && <MailEditor initial={data?.mail ?? null} onDone={() => { setEditMail(false); load() }} onCancel={() => setEditMail(false)} />}

      <h2 className="section">Webhooks</h2>
      {data && data.webhooks.length > 0 && (
        <table className="data">
          <thead><tr><th>Name</th><th>Kind</th><th>Where</th><th /></tr></thead>
          <tbody>{data.webhooks.map(w => <WebhookRow key={w.name} name={w.name} kind={w.kind} host={w.host} onChanged={load} />)}</tbody>
        </table>
      )}
      {data?.webhooks.length === 0 && <p className="muted small">No webhooks yet.</p>}
      {!newHook ? <div className="row"><button onClick={() => setNewHook(true)}>+ Add a webhook</button></div>
        : <WebhookEditor onDone={() => { setNewHook(false); load() }} onCancel={() => setNewHook(false)} />}

      <h2 className="section">Scheduled reports</h2>
      <AllReports />
    </div>
  )
}

/** Everything that leaves on a schedule from here: who made it, where it goes; pause or remove it. */
function AllReports() {
  const [list, setList] = useState<Schedule[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const load = () => api.allSchedules().then(r => setList(r.schedules), e => setError(e.message))
  useEffect(() => { load() }, [])
  const act = async (f: () => Promise<unknown>) => { try { await f(); await load() } catch (e) { setError((e as Error).message) } }
  if (error) return <div className="error">{error}</div>
  if (!list) return null
  if (!list.length) return <p className="muted small">No scheduled reports yet: anyone from the User role up can schedule a dashboard (Schedule… on it).</p>
  return (
    <div className="table-scroll"><table className="data small reports">
      <thead><tr><th>Report</th><th>Dashboard</th><th>When</th><th>To</th><th>Last run</th><th /></tr></thead>
      <tbody>{list.map(s => (
        <tr key={s.id} className={s.active ? '' : 'muted'}>
          <td><b>{s.name}</b><div className="muted">{s.alert ? `Alert: ${describeAlert(s.alert)} · ` : ''}by {s.created_by}</div></td>
          <td><a href={`/dashboards/${encodeURIComponent(s.dashboard)}`}>{s.dashboard}</a></td>
          <td>{describeWhen(s.when)}</td>
          <td>{[...s.recipients, ...s.webhooks.map(w => `#${w}`)].join(', ')}</td>
          <td>{s.last_run_at ? new Date(s.last_run_at).toLocaleString() : '–'}<div>{s.last_status}</div></td>
          <td className="nowrap">
            <button className="link" onClick={() => act(() => api.setScheduleActive(s.id, !s.active))}>{s.active ? 'Pause' : 'Resume'}</button>{' '}
            <button className="link danger" onClick={() => confirm(`Delete "${s.name}"?`) && act(() => api.deleteSchedule(s.id))}>Delete</button>
          </td>
        </tr>
      ))}</tbody>
    </table></div>
  )
}

function MailSummary({ mail, onEdit, onRemoved }: { mail: MailServerInfo; onEdit: () => void; onRemoved: () => void }) {
  const [msg, setMsg] = useState<{ kind: 'ok' | 'error'; text: string } | null>(null)
  const [busy, setBusy] = useState(false)
  const test = async () => {
    setBusy(true); setMsg(null)
    try { setMsg({ kind: 'ok', text: `Sent a test message to ${(await api.testMail()).sent_to}.` }) } catch (e) { setMsg({ kind: 'error', text: (e as Error).message }) }
    setBusy(false)
  }
  const remove = async () => {
    if (!confirm('Remove the mail server? Reports can no longer be emailed from here.')) return
    try { await api.deleteMail(); onRemoved() } catch (e) { setMsg({ kind: 'error', text: (e as Error).message }) }
  }
  return (
    <div className="card org-card">
      <p><b>{mail.from}</b></p>
      <p className="small muted mono">{mail.host}:{mail.port} · {SECURITY[mail.security]}{mail.username ? ` · ${mail.username} (password ${mail.has_password ? 'set' : 'missing'})` : ''}</p>
      <p className="small">Reports go to members{mail.allowed_domains?.length ? <> and to <b>{mail.allowed_domains.join(', ')}</b></> : ' only'}.</p>
      {msg && <div className={msg.kind === 'error' ? 'error' : 'ok'}>{msg.text}</div>}
      <div className="row">
        <button className="link danger" onClick={remove}>Remove</button>
        <button onClick={onEdit}>Edit</button>
        <button className="primary" disabled={busy} onClick={test}>{busy ? 'Sending…' : 'Send me a test'}</button>
      </div>
    </div>
  )
}

function MailEditor({ initial, onDone, onCancel }: { initial: MailServerInfo | null; onDone: () => void; onCancel: () => void }) {
  const [f, setF] = useState<MailForm>(initial
    ? { host: initial.host, port: String(initial.port), security: initial.security, username: initial.username ?? '', from: initial.from, domains: (initial.allowed_domains ?? []).join(', ') }
    : { host: '', port: '587', security: 'starttls', username: '', from: '', domains: '' })
  const [password, setPassword] = useState('')
  const [error, setError] = useState<string | null>(null)
  const keep = Boolean(initial?.has_password)
  const save = async (e: FormEvent) => {
    e.preventDefault(); setError(null)
    try { await api.saveMail(mailBody(f, password)); onDone() } catch (x) { setError((x as Error).message) }
  }
  const set = (k: keyof MailForm, v: string) => setF({ ...f, [k]: v })
  return (
    <form className="card org-card connection-form" onSubmit={save}>
      <label className="small">Sender <input value={f.from} required placeholder="Reports <reports@example.com>" onChange={e => set('from', e.target.value)} /></label>
      <label className="small">Mail server <input value={f.host} required placeholder="smtp.example.com" onChange={e => set('host', e.target.value)} /></label>
      <label className="small">Security
        <select value={f.security} onChange={e => { const s = e.target.value as MailSecurity; setF({ ...f, security: s, port: String(defaultPort(s)) }) }}>
          {(Object.keys(SECURITY) as MailSecurity[]).map(s => <option key={s} value={s}>{SECURITY[s]}</option>)}
        </select>
      </label>
      <label className="small">Port <input type="number" value={f.port} required onChange={e => set('port', e.target.value)} /></label>
      {f.security !== 'none' && <>
        <label className="small">User name <span className="muted">(optional)</span>
          <input autoComplete="off" value={f.username} onChange={e => set('username', e.target.value)} />
        </label>
        {f.username.trim() && <label className="small">Password{keep && <span className="muted"> · leave empty to keep the stored password</span>}
          <input type="password" autoComplete="new-password" value={password} required={!keep} onChange={e => setPassword(e.target.value)} />
        </label>}
      </>}
      <label className="small">Reports may also go to these email domains <span className="muted">(besides your members; e.g. a partner's)</span>
        <input value={f.domains} placeholder="partner.example" onChange={e => set('domains', e.target.value)} />
      </label>
      {error && <div className="error">{error}</div>}
      <div className="row"><button type="button" onClick={onCancel}>Cancel</button><button className="primary">Save</button></div>
    </form>
  )
}

function WebhookRow({ name, kind, host, onChanged }: { name: string; kind: WebhookKind; host: string; onChanged: () => void }) {
  const [msg, setMsg] = useState<{ kind: 'ok' | 'error'; text: string } | null>(null)
  const test = async () => {
    setMsg(null)
    try { await api.testWebhook(name); setMsg({ kind: 'ok', text: 'Sent.' }) } catch (e) { setMsg({ kind: 'error', text: (e as Error).message }) }
  }
  const remove = async () => {
    if (!confirm(`Remove the webhook "${name}"?`)) return
    try { await api.deleteWebhook(name); onChanged() } catch (e) { setMsg({ kind: 'error', text: (e as Error).message }) }
  }
  return (
    <tr>
      <td><b>{name}</b></td><td>{KINDS[kind]}</td><td className="small mono">{host}</td>
      <td>
        {msg && <span className={msg.kind === 'error' ? 'error small' : 'ok small'}>{msg.text} </span>}
        <button className="link" onClick={test}>Test</button> <button className="link danger" onClick={remove}>Remove</button>
      </td>
    </tr>
  )
}

function WebhookEditor({ onDone, onCancel }: { onDone: () => void; onCancel: () => void }) {
  const [name, setName] = useState('')
  const [kind, setKind] = useState<WebhookKind>('slack')
  const [url, setUrl] = useState('')
  const [error, setError] = useState<string | null>(null)
  const save = async (e: FormEvent) => {
    e.preventDefault(); setError(null)
    try { await api.saveWebhook(name, kind, url); onDone() } catch (x) { setError((x as Error).message) }
  }
  return (
    <form className="card org-card connection-form" onSubmit={save}>
      <label className="small">Name <input value={name} required placeholder="sales_team" onChange={e => setName(e.target.value)} /></label>
      <label className="small">Kind
        <select value={kind} onChange={e => setKind(e.target.value as WebhookKind)}>
          {(Object.keys(KINDS) as WebhookKind[]).map(k => <option key={k} value={k}>{KINDS[k]}</option>)}
        </select>
      </label>
      <label className="small">Address <span className="muted">(from Slack's or Teams' incoming webhook settings; kept secret)</span>
        <input type="password" autoComplete="off" value={url} required placeholder="https://hooks.slack.com/services/…" onChange={e => setUrl(e.target.value)} />
      </label>
      {error && <div className="error">{error}</div>}
      <div className="row"><button type="button" onClick={onCancel}>Cancel</button><button className="primary">Save</button></div>
    </form>
  )
}
