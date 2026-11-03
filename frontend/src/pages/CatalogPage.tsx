import { fieldSections, labelParts } from '../catalogView'
import { matches } from '../search'
import ListSearch from '../components/ListSearch'
import { ask } from '../dialogs'
import { useEffect, useState } from 'react'
import { Link, useSearchParams } from 'react-router-dom'
import { api } from '../api'
import type { FieldDocs, Governance, GlossaryTerm, Lineage, ModelDocs, ModelGovernance, ModelInfo, ModelStatus, SearchResult, Term } from '../api'
import { useCan } from '../auth'
import { STATUSES, describeUsage, mayDecide, mayHandOver, parseList } from '../governance'
import { forgetModels, useModels } from '../hooks'
import { parseRecipients } from '../schedule'

/** Every model: who answers for it, and how far it can be trusted. Everyone reads; its owner
 *  and Org admins decide. */
export default function CatalogPage() {
  const [list, setList] = useState<ModelGovernance[]>([])
  const [editing, setEditing] = useState<string | null>(null)
  const [documenting, setDocumenting] = useState<string | null>(null)
  const [tracing, setTracing] = useState<string | null>(null)
  // whose fields are listed; and what a link (the top bar's search) points at, picked out
  const [fieldsOf, setFieldsOf] = useState<string | null>(null)
  const [search] = useSearchParams()
  const [picked, setPicked] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const { models } = useModels()
  const can = useCan()
  const load = () => api.governance().then(r => setList(r.models), e => setError((e as Error).message))
  useEffect(() => { load() }, [])
  const label = (name: string) => models.find(m => m.name === name)?.label ?? name

  // ?model=, ?field= (with its model) or ?term=: there, picked out, once the page has it
  const wanted = search.get('term') ? `term-${search.get('term')}`
    : search.get('field') ? `field-${search.get('model')}-${search.get('field')}`
    : search.get('model') ? `model-${search.get('model')}` : null
  useEffect(() => {
    if (!wanted) return
    const t = setTimeout(() => {
      // a field: its model's list open first (this runs again once it's there)
      if (search.get('field') && fieldsOf !== search.get('model')) { setFieldsOf(search.get('model')); return }
      const el = document.getElementById(wanted)
      if (el) { el.scrollIntoView({ behavior: 'smooth', block: 'center' }); setPicked(wanted) }
    }, 300)
    return () => clearTimeout(t)
  }, [wanted, list.length, models.length, fieldsOf, search])

  return (
    <div className="page catalog">
      <h1>Data catalog</h1>
      <p className="muted">Who answers for each model, and how far it can be trusted. A model's <b>owner</b> certifies it (or marks it
        deprecated, saying why) and names <b>stewards</b> to look after its documentation. {can('manage_members')
          ? 'As an Org admin, you choose the owners.' : 'Org admins choose the owners.'}</p>
      {error && <div className="error">{error}</div>}
      <Search onModel={name => { setTracing(name); document.getElementById(`model-${name}`)?.scrollIntoView({ behavior: 'smooth' }) }} />
      <h2 className="catalog-part">Models</h2>
      {list.map(g => (
        <div key={g.model} id={`model-${g.model}`} className={`card governance${picked === `model-${g.model}` ? ' picked' : ''}`}>
          <div className="catalog-head">
            <div className="catalog-title">
              <h2>{label(g.model!)}</h2>
              {g.status && <span className={`status ${g.status}`}>{g.status}</span>}
              <span className="muted small mono">{g.model}</span>
            </div>
            <div className="catalog-actions">
              {can('explore') && <Link className="btn" to={`/explore?model=${encodeURIComponent(g.model!)}`}>Explore</Link>}
              <button aria-pressed={fieldsOf === g.model} onClick={() => setFieldsOf(fieldsOf === g.model ? null : g.model!)}>Fields</button>
              <button aria-pressed={tracing === g.model} onClick={() => setTracing(tracing === g.model ? null : g.model!)}>Lineage & usage</button>
              {g.standing !== 'reader' && <button aria-pressed={documenting === g.model} onClick={() => setDocumenting(documenting === g.model ? null : g.model!)}>Documentation</button>}
              {mayDecide(g.standing) && <button aria-pressed={editing === g.model} onClick={() => setEditing(editing === g.model ? null : g.model!)}>Edit</button>}
            </div>
          </div>
          {(() => {
            const m = models.find(x => x.name === g.model)
            return m && <>
              {m.description && <p className="catalog-description">{m.description}</p>}
              {!!m.tags?.length && <p>{m.tags.map(t => <span key={t} className="tp-badge">{t}</span>)}</p>}
            </>
          })()}
          {g.note && <p className="catalog-note">{g.note}</p>}
          <dl className="catalog-facts">
            <div><dt>Owner</dt><dd>{g.owner ?? <span className="muted">none yet</span>}</dd></div>
            {g.stewards.length > 0 && <div><dt>Stewards</dt><dd>{g.stewards.join(', ')}</dd></div>}
            {g.updated_by && <div><dt>Changed by</dt><dd>{g.updated_by}, {new Date(g.updated_at!).toLocaleDateString()}</dd></div>}
            {g.standing !== 'reader' && <div><dt>You</dt><dd>{g.standing === 'admin' ? 'Org admin' : g.standing}</dd></div>}
          </dl>
          {fieldsOf === g.model && models.some(m => m.name === g.model) && (
            <FieldsList model={models.find(m => m.name === g.model)!} picked={picked} />
          )}
          {tracing === g.model && <LineagePanel model={g.model!} info={models.find(m => m.name === g.model)} />}
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
      <Glossary picked={picked} />
    </div>
  )
}

const listText = (l?: string[]) => (l ?? []).join(', ')

const KINDS: Record<SearchResult['kind'], string> = { model: 'Model', field: 'Field', term: 'Term', dashboard: 'Dashboard' }

/** Search the catalog by names, other words and descriptions. */
function Search({ onModel }: { onModel: (name: string) => void }) {
  const [q, setQ] = useState('')
  const [results, setResults] = useState<SearchResult[] | null>(null)
  useEffect(() => {
    if (!q.trim()) return
    const ctl = new AbortController()
    const t = setTimeout(() => {
      api.searchCatalog(q, ctl.signal).then(r => setResults(r.results), () => {})
    }, 250)
    return () => { clearTimeout(t); ctl.abort() }
  }, [q])
  const shown = q.trim() ? results : null
  return (
    <div className="catalog-search">
      <input type="search" value={q} onChange={e => setQ(e.target.value)} aria-label="Search the catalog"
        placeholder="Search models, fields, terms and dashboards (e.g. takings, net revenue)…" />
      {shown && shown.length === 0 && <p className="muted small">Nothing found.</p>}
      {shown && shown.length > 0 && (
        <ul className="search-results">
          {shown.map(r => (
            <li key={`${r.kind}:${r.model}:${r.name}`}>
              <span className="tp-badge">{KINDS[r.kind]}</span>{' '}
              {r.kind === 'dashboard' ? <Link to={`/dashboards/${encodeURIComponent(r.name)}`}><b>{r.label}</b></Link>
                : r.kind === 'term' ? <a href="#glossary"><b>{r.label}</b></a>
                : <button className="link" onClick={() => onModel(r.model ?? r.name)}><b>{r.label}</b></button>}
              {r.model && <span className="muted small"> in {r.model}</span>}
              {r.text && <div className="muted small">{r.text}</div>}
            </li>
          ))}
        </ul>
      )}
    </div>
  )
}

/** A model's fields, as the catalog describes them: what each means, other names, examples, tags. */
function FieldsList({ model, picked }: { model: ModelInfo; picked: string | null }) {
  const [search, setSearch] = useState('')
  const sections = fieldSections(model, search)
  const total = model.dimensions.length + model.measures.length
  const shown = sections.flatMap(x => x.groups.flatMap(g => g.items)).length
  return (
    <div className="catalog-fields">
      <ListSearch total={total} shown={shown} value={search} onChange={setSearch} what="fields" />
      {sections.map(section => (
        <section key={section.kind} className={`catalog-section ${section.kind}`}>
          <h3>{section.title} <span className="catalog-count">{section.total}</span></h3>
          {section.groups.map(g => (
            <div key={g.group ?? ''}>
              {g.group && <h4>{g.group}</h4>}
              {g.items.map(({ field: f, label }) => {
                const id = `field-${model.name}-${f.name}`
                return (
                  <div key={f.name} id={id} className={`catalog-field${picked === id ? ' picked' : ''}`}>
                    <div className="catalog-field-name">
                      <b>{label}</b>
                      <span className="mono">{f.name}</span>
                      {!!f.tags?.length && <span>{f.tags.map(t => <span key={t} className="tp-badge">{t}</span>)}</span>}
                    </div>
                    <div className="catalog-field-about">
                      {f.description ? <p>{f.description}</p> : <p className="catalog-none">No description yet</p>}
                      {!!f.synonyms?.length && <p className="small"><b>Also called</b> {f.synonyms.join(', ')}</p>}
                      {!!f.examples?.length && <p className="small"><b>For example</b> {f.examples.join('; ')}</p>}
                    </div>
                  </div>
                )
              })}
            </div>
          ))}
        </section>
      ))}
    </div>
  )
}

/** Where a model comes from and where it's used. */
function LineagePanel({ model, info }: { model: string; info?: ModelInfo }) {
  const [l, setL] = useState<Lineage | null>(null)
  const [error, setError] = useState<string | null>(null)
  useEffect(() => { api.lineage(model).then(setL, e => setError((e as Error).message)) }, [model])
  if (error) return <div className="error">{error}</div>
  if (!l) return <p className="muted small">Loading…</p>
  const all = [...(info?.dimensions ?? []), ...(info?.measures ?? [])]
  const label = (f: string) => all.find(x => x.name === f)?.label ?? f
  const unused = all.filter(f => !l?.fields[f.name]).map(f => f.label ?? f.name)
  return (
    <div className="lineage">
      <h3>Lineage & usage</h3>
      <dl className="catalog-rows">
        <dt>Reads</dt>
        <dd>{l.reads.connection ? <>the connection <span className="mono">{l.reads.connection}</span></> : 'its models, joined'}
          {l.reads.table && <>, table <span className="mono">{l.reads.table}</span></>}
          {l.reads.sql && <> (its own SQL)</>}
          {l.reads.extends && <>, extending <span className="mono">{l.reads.extends}</span></>}</dd>
        <dt>Usage</dt>
        <dd>{describeUsage(l.usage)}{l.usage.last && `; last ${new Date(l.usage.last).toLocaleDateString()}`}</dd>
        <dt>Dashboards</dt>
        <dd>{l.dashboards.length ? l.dashboards.map((d, i) => <span key={d.id}>{i > 0 && ', '}<Link to={`/dashboards/${encodeURIComponent(d.id)}`}>{d.title}</Link>
          {' '}<span className="muted">({d.tiles.length} {d.tiles.length === 1 ? 'tile' : 'tiles'})</span></span>) : <span className="muted">none</span>}</dd>
        {l.reports > 0 && <><dt>Reports</dt><dd>{l.reports} scheduled</dd></>}
        {l.blends.length > 0 && <><dt>Blends</dt><dd className="mono">{l.blends.join(', ')}</dd></>}
        {l.formulas.length > 0 && <><dt>Shared formulas</dt><dd className="mono">{l.formulas.join(', ')}</dd></>}
      </dl>
      {unused.length > 0 && (
        <details className="catalog-unused">
          <summary>{unused.length} {unused.length === 1 ? 'field' : 'fields'} on no dashboard</summary>
          <p className="small">{unused.join(', ')}</p>
        </details>
      )}
      {Object.keys(l.fields).length > 0 && (
        <table className="docs-fields catalog-uses">
          <thead><tr><th>Field</th><th>Used in</th></tr></thead>
          <tbody>
            {Object.entries(l.fields).map(([f, places]) => {
              const [group, name] = labelParts(label(f))
              return <tr key={f}><td><b>{name}</b>{group && <div className="muted small">{group}</div>}</td><td className="small">{places.join(', ')}</td></tr>
            })}
          </tbody>
        </table>
      )}
    </div>
  )
}

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
  const [fieldSearch, setFieldSearch] = useState('')
  const fieldOf = (name: string) => [...model.dimensions, ...model.measures].find(x => x.name === name)
  // what's been written is found too; a hidden field keeps what was typed in it
  const shownFields = Object.entries(fields).filter(([name, f]) => matches(fieldSearch, name, fieldOf(name)?.label, f.description))
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
        <textarea rows={3} maxLength={2000} value={description} placeholder={model.description ?? ''} onChange={e => setDescription(e.target.value)} />
      </label>
      <label>Tags <span className="muted small">(such as finance, pii; separated by commas)</span>
        <input value={tags} onChange={e => setTags(e.target.value)} />
      </label>
      <ListSearch total={Object.keys(fields).length} shown={shownFields.length} value={fieldSearch} onChange={setFieldSearch} what="fields" />
      <div className="docs-edit">
        {shownFields.map(([name, f]) => {
          const field = fieldOf(name)
          return (
            <div key={name} className="docs-field">
              <div><b>{field?.label ?? name}</b> <span className="muted small mono">{name}</span></div>
              <label className="docs-description">Description
                <textarea rows={2} value={f.description} placeholder={field?.description ?? ''} onChange={e => set(name, 'description', e.target.value)} /></label>
              <label>Also called<input value={f.synonyms} onChange={e => set(name, 'synonyms', e.target.value)} /></label>
              <label>Examples<input value={f.examples} onChange={e => set(name, 'examples', e.target.value)} /></label>
              <label>Tags<input value={f.tags} onChange={e => set(name, 'tags', e.target.value)} /></label>
              <label>Glossary terms<input value={f.terms} onChange={e => set(name, 'terms', e.target.value)} /></label>
            </div>
          )
        })}
      </div>
      {error && <div className="error">{error}</div>}
      <div className="row"><button onClick={onCancel}>Cancel</button><button className="primary" onClick={save}>Save</button></div>
    </div>
  )
}

/** The organisation's business terms; a term belongs to whoever added it. */
function Glossary({ picked }: { picked: string | null }) {
  const [terms, setTerms] = useState<GlossaryTerm[]>([])
  const [mayAdd, setMayAdd] = useState(false)
  const [editing, setEditing] = useState<GlossaryTerm | 'new' | null>(null)
  const [error, setError] = useState<string | null>(null)
  const load = () => api.glossary().then(r => { setTerms(r.terms); setMayAdd(r.may_add) }, e => setError((e as Error).message))
  useEffect(() => { load() }, [])
  const remove = async (t: GlossaryTerm) => {
    if (!(await ask.confirm(`Remove "${t.name}" from the glossary?`, { danger: true, ok: 'Remove' }))) return
    try { await api.deleteTerm(t.name); load() } catch (e) { setError((e as Error).message) }
  }
  return <section className="glossary" id="glossary">
    <h2 className="catalog-part">Glossary</h2>
    <p className="muted">The organisation's business terms, and the fields that implement each (linked from a model's documentation).
      Agents read them too.</p>
    {error && <div className="error">{error}</div>}
    {mayAdd && editing !== 'new' && <div className="row"><button onClick={() => setEditing('new')}>+ New term</button></div>}
    {editing === 'new' && <TermForm onCancel={() => setEditing(null)} onSave={async t => { await api.saveTerm(t); setEditing(null); load() }} />}
    {terms.length === 0 && <p className="muted small">No terms yet.</p>}
    {terms.map(t => (
      <div key={t.name} id={`term-${t.name}`} className={`card governance${picked === `term-${t.name}` ? ' picked' : ''}`}>
        <div className="catalog-head">
          <div className="catalog-title"><h3>{t.name}</h3></div>
          {t.may_edit && editing !== t && <div className="catalog-actions"><button onClick={() => setEditing(t)}>Edit</button><button className="danger" onClick={() => remove(t)}>Delete</button></div>}
        </div>
        <p className="catalog-description">{t.definition}</p>
        <dl className="catalog-facts">
          {!!t.synonyms?.length && <div><dt>Also called</dt><dd>{t.synonyms.join(', ')}</dd></div>}
          {t.fields.length > 0 && <div><dt>Fields</dt><dd className="mono">{t.fields.join(', ')}</dd></div>}
          {t.owner && <div><dt>Owner</dt><dd>{t.owner}</dd></div>}
        </dl>
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
      <label>Definition <textarea rows={3} maxLength={2000} value={definition} onChange={e => setDefinition(e.target.value)} /></label>
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
        <textarea rows={3} maxLength={500} value={note} onChange={e => setNote(e.target.value)} />
      </label>
      {error && <div className="error">{error}</div>}
      <div className="row"><button onClick={onCancel}>Cancel</button><button className="primary" onClick={save}>Save</button></div>
    </div>
  )
}
