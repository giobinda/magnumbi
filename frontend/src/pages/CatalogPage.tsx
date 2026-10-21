import { useEffect, useState } from 'react'
import { Link } from 'react-router-dom'
import { api } from '../api'
import type { FieldDocs, Governance, GlossaryTerm, ModelDocs, ModelGovernance, ModelInfo, ModelStatus, Term } from '../api'
import { useCan } from '../auth'
import { STATUSES, mayDecide, mayHandOver, parseList } from '../governance'
import { forgetModels, useModels } from '../hooks'
import { parseRecipients } from '../schedule'

/** Every model: who answers for it, and how far it can be trusted. Everyone reads; its owner
 *  and Org admins decide. */
export default function CatalogPage() {
  const [list, setList] = useState<ModelGovernance[]>([])
  const [editing, setEditing] = useState<string | null>(null)
  const [documenting, setDocumenting] = useState<string | null>(null)
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
            {g.standing !== 'reader' && documenting !== g.model && <button onClick={() => setDocumenting(g.model!)}>Documentation</button>}
            {mayDecide(g.standing) && editing !== g.model && <button onClick={() => setEditing(g.model!)}>Edit</button>}
          </div>
          {g.note && <p className="small">{g.note}</p>}
          {(() => {
            const m = models.find(x => x.name === g.model)
            return m && <>
              {m.description && <p className="small">{m.description}</p>}
              {!!m.tags?.length && <p>{m.tags.map(t => <span key={t} className="tp-badge">{t}</span>)}</p>}
            </>
          })()}
          <p className="muted small">
            {g.owner ? <>Owner: <b>{g.owner}</b></> : 'No owner yet'}
            {g.stewards.length > 0 && <> · Stewards: {g.stewards.join(', ')}</>}
            {g.updated_by && <> · changed by {g.updated_by}, {new Date(g.updated_at!).toLocaleDateString()}</>}
            {g.standing !== 'reader' && <> · you: {g.standing === 'admin' ? 'Org admin' : g.standing}</>}
          </p>
          {documenting === g.model && models.some(m => m.name === g.model) && (
            <DocsForm model={models.find(m => m.name === g.model)!} initial={g.docs ?? {}} onCancel={() => setDocumenting(null)}
              onSave={async d => { await api.saveDocs(g.model!, d); setDocumenting(null); forgetModels(); load() }} />
          )}
          {editing === g.model && (
            <GovernanceForm initial={g} onCancel={() => setEditing(null)}
              onSave={async next => { await api.saveGovernance(g.model!, next); setEditing(null); forgetModels(); load() }} />
          )}
        </div>
      ))}
      <Glossary />
    </div>
  )
}

const listText = (l?: string[]) => (l ?? []).join(', ')

/** What the organisation says about a model and its fields (the model's own words stay). */
function DocsForm({ model, initial, onSave, onCancel }: {
  model: ModelInfo; initial: ModelDocs; onSave: (d: ModelDocs) => Promise<void>; onCancel: () => void
}) {
  const [description, setDescription] = useState(initial.description ?? '')
  const [tags, setTags] = useState(listText(initial.tags))
  const [fields, setFields] = useState<Record<string, Record<keyof FieldDocs, string>>>(() => Object.fromEntries(
    [...model.dimensions, ...model.measures].map(f => {
      const d = initial.fields?.[f.name] ?? {}
      return [f.name, { description: d.description ?? '', synonyms: listText(d.synonyms), examples: listText(d.examples),
        tags: listText(d.tags), terms: listText(d.terms) }]
    })))
  const [error, setError] = useState<string | null>(null)
  const set = (name: string, key: keyof FieldDocs, value: string) => setFields(fs => ({ ...fs, [name]: { ...fs[name], [key]: value } }))
  const save = async () => {
    setError(null)
    const out: Record<string, FieldDocs> = {}
    for (const [name, f] of Object.entries(fields)) {
      const d: FieldDocs = {
        description: f.description.trim() || null, synonyms: parseList(f.synonyms), examples: parseList(f.examples),
        tags: parseList(f.tags.toLowerCase()), terms: parseList(f.terms),
      }
      if (d.description || d.synonyms!.length || d.examples!.length || d.tags!.length || d.terms!.length) out[name] = d
    }
    try {
      await onSave({ description: description.trim() || null, tags: parseList(tags.toLowerCase()), fields: out })
    } catch (e) { setError((e as Error).message) }
  }
  return (
    <div className="governance-form docs-form">
      <label>Description <span className="muted small">(replaces the model's own here)</span>
        <textarea rows={2} maxLength={2000} value={description} placeholder={model.description ?? ''} onChange={e => setDescription(e.target.value)} />
      </label>
      <label>Tags <span className="muted small">(such as finance, pii; separated by commas)</span>
        <input value={tags} onChange={e => setTags(e.target.value)} />
      </label>
      <table className="docs-fields">
        <thead><tr><th>Field</th><th>Description</th><th>Also called</th><th>Examples</th><th>Tags</th><th>Glossary terms</th></tr></thead>
        <tbody>
          {Object.entries(fields).map(([name, f]) => {
            const field = [...model.dimensions, ...model.measures].find(x => x.name === name)
            return (
              <tr key={name}>
                <td><b>{field?.label ?? name}</b><div className="muted small mono">{name}</div></td>
                <td><input value={f.description} placeholder={field?.description ?? ''} onChange={e => set(name, 'description', e.target.value)} /></td>
                <td><input value={f.synonyms} onChange={e => set(name, 'synonyms', e.target.value)} /></td>
                <td><input value={f.examples} onChange={e => set(name, 'examples', e.target.value)} /></td>
                <td><input value={f.tags} onChange={e => set(name, 'tags', e.target.value)} /></td>
                <td><input value={f.terms} onChange={e => set(name, 'terms', e.target.value)} /></td>
              </tr>
            )
          })}
        </tbody>
      </table>
      {error && <div className="error">{error}</div>}
      <div className="row"><button onClick={onCancel}>Cancel</button><button className="primary" onClick={save}>Save</button></div>
    </div>
  )
}

/** The organisation's business terms; a term belongs to whoever added it. */
function Glossary() {
  const [terms, setTerms] = useState<GlossaryTerm[]>([])
  const [mayAdd, setMayAdd] = useState(false)
  const [editing, setEditing] = useState<GlossaryTerm | 'new' | null>(null)
  const [error, setError] = useState<string | null>(null)
  const load = () => api.glossary().then(r => { setTerms(r.terms); setMayAdd(r.may_add) }, e => setError((e as Error).message))
  useEffect(() => { load() }, [])
  const remove = async (t: GlossaryTerm) => {
    if (!confirm(`Remove "${t.name}" from the glossary?`)) return
    try { await api.deleteTerm(t.name); load() } catch (e) { setError((e as Error).message) }
  }
  return <section className="glossary">
    <h2 className="section">Glossary</h2>
    <p className="muted small">The organisation's business terms, and the fields that implement each (linked from a model's documentation).
      Agents read them too.</p>
    {error && <div className="error">{error}</div>}
    {mayAdd && editing !== 'new' && <div className="row"><button onClick={() => setEditing('new')}>+ New term</button></div>}
    {editing === 'new' && <TermForm onCancel={() => setEditing(null)} onSave={async t => { await api.saveTerm(t); setEditing(null); load() }} />}
    {terms.length === 0 && <p className="muted small">No terms yet.</p>}
    {terms.map(t => (
      <div key={t.name} className="card governance">
        <div className="row wrap">
          <b>{t.name}</b>
          {!!t.synonyms?.length && <span className="muted small">also: {t.synonyms.join(', ')}</span>}
          <span className="grow" />
          {t.may_edit && editing !== t && <><button onClick={() => setEditing(t)}>Edit</button><button className="danger" onClick={() => remove(t)}>Delete</button></>}
        </div>
        <p className="small">{t.definition}</p>
        <p className="muted small">{t.fields.length ? <>Fields: <span className="mono">{t.fields.join(', ')}</span> · </> : ''}
          {t.owner ? `Owner: ${t.owner}` : ''}</p>
        {editing === t && <TermForm initial={t} onCancel={() => setEditing(null)} onSave={async x => { await api.saveTerm(x); setEditing(null); load() }} />}
      </div>
    ))}
  </section>
}

function TermForm({ initial, onSave, onCancel }: { initial?: Term; onSave: (t: Term) => Promise<void>; onCancel: () => void }) {
  const [name, setName] = useState(initial?.name ?? '')
  const [definition, setDefinition] = useState(initial?.definition ?? '')
  const [synonyms, setSynonyms] = useState(listText(initial?.synonyms))
  const [error, setError] = useState<string | null>(null)
  const save = async () => {
    setError(null)
    try { await onSave({ name: name.trim(), definition: definition.trim(), synonyms: parseList(synonyms) }) } catch (e) { setError((e as Error).message) }
  }
  return (
    <div className="governance-form">
      <label>Term <input value={name} disabled={Boolean(initial)} placeholder="e.g. Net revenue" onChange={e => setName(e.target.value)} /></label>
      <label>Definition <textarea rows={2} maxLength={2000} value={definition} onChange={e => setDefinition(e.target.value)} /></label>
      <label>Also called <span className="muted small">(separated by commas)</span>
        <input value={synonyms} onChange={e => setSynonyms(e.target.value)} /></label>
      {error && <div className="error">{error}</div>}
      <div className="row"><button onClick={onCancel}>Cancel</button><button className="primary" disabled={!name.trim() || !definition.trim()} onClick={save}>Save</button></div>
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
