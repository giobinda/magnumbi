import { useEffect, useState } from 'react'
import { Link } from 'react-router-dom'
import { api } from '../api'
import type { Governance, ModelGovernance, ModelStatus } from '../api'
import { useCan } from '../auth'
import { STATUSES, mayDecide, mayHandOver } from '../governance'
import { forgetModels, useModels } from '../hooks'
import { parseRecipients } from '../schedule'

/** Every model: who answers for it, and how far it can be trusted. Everyone reads; its owner
 *  and Org admins decide. */
export default function CatalogPage() {
  const [list, setList] = useState<ModelGovernance[]>([])
  const [editing, setEditing] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const { models } = useModels()
  const can = useCan()
  const load = () => api.governance().then(r => setList(r.models), e => setError((e as Error).message))
  useEffect(() => { load() }, [])
  const label = (name: string) => models.find(m => m.name === name)?.label ?? name

  return (
    <div className="page narrow catalog">
      <h1>Data catalog</h1>
      <p className="muted">Who answers for each model, and how far it can be trusted. A model's <b>owner</b> certifies it (or marks it
        deprecated, saying why) and names <b>stewards</b> to look after its documentation. {can('manage_members')
          ? 'As an Org admin, you choose the owners.' : 'Org admins choose the owners.'}</p>
      {error && <div className="error">{error}</div>}
      {list.map(g => (
        <div key={g.model} className="card governance">
          <div className="row wrap">
            <b>{label(g.model!)}</b>
            {g.status && <span className={`status ${g.status}`}>{g.status}</span>}
            <span className="muted small mono">{g.model}</span>
            <span className="grow" />
            {can('explore') && <Link className="btn" to={`/explore?model=${encodeURIComponent(g.model!)}`}>Explore</Link>}
            {mayDecide(g.standing) && editing !== g.model && <button onClick={() => setEditing(g.model!)}>Edit</button>}
          </div>
          {g.note && <p className="small">{g.note}</p>}
          <p className="muted small">
            {g.owner ? <>Owner: <b>{g.owner}</b></> : 'No owner yet'}
            {g.stewards.length > 0 && <> · Stewards: {g.stewards.join(', ')}</>}
            {g.updated_by && <> · changed by {g.updated_by}, {new Date(g.updated_at!).toLocaleDateString()}</>}
            {g.standing !== 'reader' && <> · you: {g.standing === 'admin' ? 'Org admin' : g.standing}</>}
          </p>
          {editing === g.model && (
            <GovernanceForm initial={g} onCancel={() => setEditing(null)}
              onSave={async next => { await api.saveGovernance(g.model!, next); setEditing(null); forgetModels(); load() }} />
          )}
        </div>
      ))}
    </div>
  )
}

function GovernanceForm({ initial, onSave, onCancel }: {
  initial: ModelGovernance; onSave: (g: Governance) => Promise<void>; onCancel: () => void
}) {
  const [owner, setOwner] = useState(initial.owner ?? '')
  const [stewards, setStewards] = useState(initial.stewards.join(', '))
  const [status, setStatus] = useState<ModelStatus | ''>(initial.status ?? '')
  const [note, setNote] = useState(initial.note ?? '')
  const [error, setError] = useState<string | null>(null)
  const save = async () => {
    setError(null)
    try {
      await onSave({
        owner: owner.trim().toLowerCase() || null, stewards: parseRecipients(stewards),
        status: status || null, note: note.trim() || null,
      })
    } catch (e) { setError((e as Error).message) }
  }
  return (
    <div className="governance-form">
      <label>Owner
        <input type="email" value={owner} disabled={!mayHandOver(initial.standing)} placeholder="name@company.com"
          title={mayHandOver(initial.standing) ? undefined : 'Org admins choose the owner'} onChange={e => setOwner(e.target.value)} />
      </label>
      <label>Stewards <span className="muted small">(emails of members, separated by commas)</span>
        <input value={stewards} onChange={e => setStewards(e.target.value)} placeholder="name@company.com, …" />
      </label>
      <label>Status
        <select value={status} onChange={e => setStatus(e.target.value as ModelStatus | '')}>
          <option value="">not reviewed</option>
          {STATUSES.map(s => <option key={s} value={s}>{s}</option>)}
        </select>
      </label>
      <label>Note <span className="muted small">(why it's certified, or what to use instead)</span>
        <textarea rows={2} maxLength={500} value={note} onChange={e => setNote(e.target.value)} />
      </label>
      {error && <div className="error">{error}</div>}
      <div className="row"><button onClick={onCancel}>Cancel</button><button className="primary" onClick={save}>Save</button></div>
    </div>
  )
}
