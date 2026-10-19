import { useEffect, useState } from 'react'
import { api } from '../api'
import type { ReportFormat, Role, Schedule, ScheduleWhen, ShareLink, TileRequest } from '../api'
import { WEEKDAYS, describeWhen, parseRecipients } from '../schedule'

const FORMATS: { format: ReportFormat; label: string; detail: string }[] = [
  { format: 'csv', label: 'The data (zip of CSV files)', detail: 'Each chart\'s data, attached to the email. Webhooks get a link to the dashboard.' },
  { format: 'signed', label: 'A guest link with a password', detail: 'A fresh link each time, valid until the next report (at most 14 days). Recipients need the password you set here: it is never in the email.' },
  { format: 'public', label: 'A public link', detail: 'One of this dashboard\'s public links: anyone with it can see the dashboard.' },
]

/** Scheduled reports of a dashboard: when, as what, to whom (Users and up). */
export default function ScheduleDialog({ id, me, myRole, isAdmin, inOrg, request, onClose }: {
  id: string; me: string; myRole: Role; isAdmin: boolean; inOrg: boolean; request: TileRequest; onClose: () => void
}) {
  const [list, setList] = useState<Schedule[]>([])
  const [webhooks, setWebhooks] = useState<string[]>([])
  const [mail, setMail] = useState(true)
  const [publicLinks, setPublicLinks] = useState<ShareLink[]>([])
  const [name, setName] = useState('Weekly report')
  const [when, setWhen] = useState<ScheduleWhen>({ every: 'weekly', day: 1, at: '08:00', timezone: Intl.DateTimeFormat().resolvedOptions().timeZone })
  const [format, setFormat] = useState<ReportFormat>('csv')
  const [recipients, setRecipients] = useState(me)
  const [hooks, setHooks] = useState<string[]>([])
  const [password, setPassword] = useState('')
  const [shareLink, setShareLink] = useState('')
  const [made, setMade] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  const load = () => api.schedules(id).then(r => { setList(r.schedules); setWebhooks(r.webhooks); setMail(r.mail) }, e => setError(e.message))
  useEffect(() => {
    load()
    if (isAdmin && inOrg) api.shareLinks(id).then(r => setPublicLinks(r.links), () => {})
  }, [id])  // eslint-disable-line react-hooks/exhaustive-deps

  const powerUser = ['power_user', 'org_admin', 'system_owner'].includes(myRole)
  const formats = FORMATS.filter(f => f.format === 'csv' || (inOrg && (f.format === 'signed' ? powerUser : isAdmin)))

  const create = async () => {
    setBusy(true); setError(null); setMade(null)
    try {
      const s = await api.createSchedule(id, {
        name, when: { ...when, day: when.every === 'daily' ? null : when.day ?? 1 }, format,
        recipients: parseRecipients(recipients), webhooks: hooks, request: { ...request, drill: [] },
        ...(format === 'signed' ? { password } : {}), ...(format === 'public' ? { share_link: shareLink } : {}),
      })
      setMade(s.password ? `The guests' password is ${s.password}: send it to them separately. It isn't shown again.` : `Scheduled: ${describeWhen(s.when)}.`)
      setPassword(''); await load()
    } catch (e) { setError((e as Error).message) } finally { setBusy(false) }
  }
  const act = async (f: () => Promise<unknown>) => { setError(null); try { await f(); await load() } catch (e) { setError((e as Error).message) } }

  return (
    <div className="modal-bg" onMouseDown={e => { if (e.target === e.currentTarget) onClose() }}>
      <div className="modal share-dialog">
        <h2>Scheduled reports</h2>
        <p className="small">Send this dashboard on a schedule, with the filters and period on screen now, to your organisation's
          members (and email domains your Org admins allow) and to its Slack or Teams webhooks. It is sent as you: your own
          restrictions apply, and it stops if you can no longer share it.</p>
        {!mail && <div className="warn small">There is no mail server yet: reports can go to webhooks only until an Org admin sets one up (Delivery).</div>}

        {list.length > 0 && (
          <div className="table-scroll"><table className="data small reports">
            <thead><tr><th>Report</th><th>When</th><th>To</th><th>Last run</th></tr></thead>
            <tbody>{list.map(s => (
              <tr key={s.id} className={s.active ? '' : 'muted'}>
                <td><b>{s.name}</b><div className="muted">{FORMATS.find(f => f.format === s.format)?.label}</div>
                  <div className="nowrap">
                    <button className="link" onClick={() => act(() => api.runSchedule(s.id))}>Send now</button>{' · '}
                    <button className="link" onClick={() => act(() => api.setScheduleActive(s.id, !s.active))}>{s.active ? 'Pause' : 'Resume'}</button>{' · '}
                    <button className="link danger" onClick={() => confirm(`Delete "${s.name}"?`) && act(() => api.deleteSchedule(s.id))}>Delete</button>
                  </div>
                </td>
                <td>{describeWhen(s.when)}{s.active && <div className="muted">next {new Date(s.next_run_at).toLocaleString()}</div>}</td>
                <td>{[...s.recipients, ...s.webhooks.map(w => `#${w}`)].join(', ')}</td>
                <td>{s.last_run_at ? new Date(s.last_run_at).toLocaleString() : '–'}<div className={s.last_status?.startsWith('sent') ? 'muted' : 'error-text'}>{s.last_status}</div></td>
              </tr>
            ))}</tbody>
          </table></div>
        )}

        <h3>New report</h3>
        <label className="small">Name <input value={name} onChange={e => setName(e.target.value)} /></label>
        <div className="row wrap">
          <label className="small">Every
            <select value={when.every} onChange={e => setWhen({ ...when, every: e.target.value as ScheduleWhen['every'], day: 1 })}>
              <option value="daily">day</option><option value="weekly">week</option><option value="monthly">month</option>
            </select>
          </label>
          {when.every === 'weekly' && <label className="small">on
            <select value={when.day ?? 1} onChange={e => setWhen({ ...when, day: Number(e.target.value) })}>
              {WEEKDAYS.map((d, i) => <option key={d} value={i + 1}>{d}</option>)}
            </select>
          </label>}
          {when.every === 'monthly' && <label className="small">on day
            <select value={when.day ?? 1} onChange={e => setWhen({ ...when, day: Number(e.target.value) })}>
              {Array.from({ length: 28 }, (_, i) => <option key={i} value={i + 1}>{i + 1}</option>)}
            </select>
          </label>}
          <label className="small">at <input type="time" value={when.at} onChange={e => setWhen({ ...when, at: e.target.value })} /></label>
          <label className="small">time zone <input value={when.timezone} onChange={e => setWhen({ ...when, timezone: e.target.value })} /></label>
        </div>
        <fieldset className="small">
          <legend>What it sends</legend>
          {formats.map(f => (
            <label key={f.format} className="check"><input type="radio" checked={format === f.format} onChange={() => setFormat(f.format)} />
              <span><b>{f.label}</b><br /><span className="muted">{f.detail}</span></span></label>
          ))}
        </fieldset>
        {format === 'signed' && <label className="small">Guests' password <span className="muted">(at least 10 characters; empty: one is made up and shown once)</span>
          <input type="password" autoComplete="new-password" value={password} onChange={e => setPassword(e.target.value)} /></label>}
        {format === 'public' && <label className="small">Public link
          <select value={shareLink} onChange={e => setShareLink(e.target.value)}>
            <option value="">{publicLinks.length ? 'Choose one' : 'None yet: make one with Share publicly'}</option>
            {publicLinks.map(l => <option key={l.id} value={l.id}>{l.url}</option>)}
          </select>
        </label>}
        <label className="small">Email to <span className="muted">(members, or domains your Org admins allow; separated by commas)</span>
          <textarea rows={2} value={recipients} onChange={e => setRecipients(e.target.value)} /></label>
        {webhooks.length > 0 && <div className="small">Webhooks {webhooks.map(w => (
          <label key={w} className="check inline"><input type="checkbox" checked={hooks.includes(w)}
            onChange={e => setHooks(e.target.checked ? [...hooks, w] : hooks.filter(x => x !== w))} /> {w}</label>
        ))}</div>}
        {made && <div className="ok">{made}</div>}
        {error && <div className="error">{error}</div>}
        <div className="row">
          <button onClick={onClose}>Close</button>
          <button className="primary" disabled={busy || !name.trim()} onClick={create}>{busy ? 'Saving…' : 'Schedule'}</button>
        </div>
      </div>
    </div>
  )
}
