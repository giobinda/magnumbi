import { useEffect, useState } from 'react'
import { Link, useNavigate } from 'react-router-dom'
import { api } from '../api'
import type { ConnectionInfo, Formula, SharedFormula } from '../api'
import FormulaEditor from '../components/FormulaEditor'
import { forgetModels, useModels } from '../hooks'
import { DIMENSION_TYPES, MEASURE_TYPES, modelFrom, rowsFor } from '../modelEditor'
import type { FieldRow, MeasureDef, ModelDef } from '../modelEditor'
import { starterDashboard } from '../starter'

const slug = (s: string) => s.toLowerCase().replace(/[^a-z0-9_]+/g, '_').replace(/^_+|_+$/g, '').replace(/^(\d)/, 't_$1') || 'model'

/** Org admins: the organisation's own models, made from its connections' tables. */
export default function ModelsPage() {
  const [own, setOwn] = useState<{ model: ModelDef; updated_at: string; updated_by: string | null }[]>([])
  const [editing, setEditing] = useState<ModelDef | 'new' | null>(null)
  const [error, setError] = useState<string | null>(null)
  const { models } = useModels()
  const navigate = useNavigate()
  const load = () => api.orgModels().then(r => setOwn(r.models), e => setError(e.message))

  // a first dashboard on a model, to change at will
  const starter = async (name: string) => {
    const m = models.find(x => x.name === name)
    if (!m) { setError('This model is still loading (or has a problem): try again in a moment.'); return }
    try {
      const taken = (await api.dashboards()).dashboards.map(d => d.id)
      let id = `${name}_overview`, n = 2
      while (taken.includes(id)) id = `${name}_overview_${n++}`
      await api.saveDashboard(id, starterDashboard(m), 'none')
      navigate(`/dashboards/${id}`)
    } catch (e) { setError((e as Error).message) }
  }
  useEffect(() => { load() }, [])

  const remove = async (m: ModelDef) => {
    if (!confirm(`Delete the model "${m.label ?? m.name}"? Dashboards and blends using it stop working.`)) return
    try { await api.deleteOrgModel(m.name); forgetModels(); load() } catch (e) { setError((e as Error).message) }
  }
  if (editing) {
    return <ModelWizard initial={editing === 'new' ? null : editing} taken={models.map(m => m.name)}
      onDone={() => { setEditing(null); forgetModels(); load() }} onCancel={() => setEditing(null)} />
  }
  const platform = models.filter(m => !m.own && !m.blend)
  return (
    <div className="page narrow">
      <h1>Models</h1>
      <p className="muted">A model says what your data means: which table, the fields people group and filter by (dimensions), and
        the numbers they add up (measures). Make one from any table of your connections; everyone in your organisation can then
        explore it and build dashboards on it.</p>
      {error && <div className="error">{error}</div>}
      <div className="row"><button className="primary" onClick={() => setEditing('new')}>+ New model</button></div>

      <h2 className="section">Your organisation's models</h2>
      {own.length === 0 && <p className="muted small">None yet: start with <b>New model</b>.</p>}
      {own.map(({ model: m, updated_at, updated_by }) => (
        <div key={m.name} className="card person row wrap">
          <b>{m.label ?? m.name}</b>
          <span className="muted small mono">{m.name} · {m.connection} · {m.table} · {m.dimensions.length} dimensions · {m.measures.length} measures</span>
          <span className="grow" />
          <span className="muted small">{updated_by ? `by ${updated_by}, ` : ''}{new Date(updated_at).toLocaleDateString()}</span>
          <Link className="btn" to={`/explore?model=${encodeURIComponent(m.name)}`}>Explore</Link>
          <button onClick={() => starter(m.name)}>Starter dashboard</button>
          <button onClick={() => setEditing(m)}>Edit</button>
          <button className="danger" onClick={() => remove(m)}>Delete</button>
        </div>
      ))}

      {platform.length > 0 && <>
        <h2 className="section">From the platform</h2>
        <p className="muted small">Defined by your Magnum BI platform's team, in its project files.</p>
        {platform.map(m => (
          <div key={m.name} className="card person row wrap">
            <span>{m.label}</span><span className="muted small mono">{m.name}</span>
            <span className="grow" />
            <button onClick={() => starter(m.name)}>Starter dashboard</button>
          </div>
        ))}
      </>}
      <SharedFormulas />
      <p className="muted small">Restrictions on what members see apply to models by name, and follow the same table into any new model
        on it. A model on a different table or view is different data: check <Link to="/members">Members</Link> after adding one.</p>
    </div>
  )
}

/** New model: connection, table, then the fields (proposed from its columns). Or edit one. */
function ModelWizard({ initial, taken, onDone, onCancel }: { initial: ModelDef | null; taken: string[]; onDone: () => void; onCancel: () => void }) {
  const [connections, setConnections] = useState<ConnectionInfo[]>([])
  const [tables, setTables] = useState<string[] | null>(null)
  const [connection, setConnection] = useState(initial?.connection ?? '')
  const [table, setTable] = useState(initial?.table ?? '')
  const [name, setName] = useState(initial?.name ?? '')
  const [label, setLabel] = useState(initial?.label ?? '')
  const [rows, setRows] = useState<FieldRow[] | null>(null)
  const [count, setCount] = useState<MeasureDef>({ name: 'count', label: 'Count', type: 'count' })
  const [skipped, setSkipped] = useState<string[]>([])
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  useEffect(() => { api.connections().then(r => setConnections(r.connections), e => setError(e.message)) }, [])
  const pickConnection = (c: string) => {
    setConnection(c); setTables(null); setError(null)
    if (c) api.tables(c).then(r => setTables(r.tables), e => { setTables([]); setError(e.message) })
  }

  const review = async (t: string) => {
    setError(null); setBusy(true)
    try {
      const n = initial?.name ?? (name || slug(t.split('.').pop() ?? t))
      const p = await api.proposeModel(connection, t, n)
      setName(n); setLabel(initial?.label ?? p.model.label ?? n); setTable(t); setSkipped(p.skipped)
      setRows(rowsFor(p.model, initial ?? undefined))
      setCount(initial?.measures.find(m => m.type === 'count' && !m.sql) ?? p.model.measures[0])
    } catch (e) { setError((e as Error).message) } finally { setBusy(false) }
  }
  // editing: straight to the fields
  useEffect(() => { if (initial) review(initial.table ?? '') }, [])  // eslint-disable-line react-hooks/exhaustive-deps
  const save = async () => {
    if (!rows) return
    setError(null); setBusy(true)
    const m = modelFrom({ name, label, connection, table }, rows, count)
    try { await api.saveOrgModel(m, true); await api.saveOrgModel(m); onDone() } catch (e) { setError((e as Error).message) } finally { setBusy(false) }
  }
  const set = (i: number, patch: Partial<FieldRow>) => setRows(rs => rs!.map((r, j) => (j === i ? { ...r, ...patch } : r)))
  const nameTaken = !initial && taken.includes(name)

  return (
    <div className="page narrow model-wizard">
      <h1>{initial ? `Edit ${initial.label ?? initial.name}` : 'New model'}</h1>
      {error && <div className="error">{error}</div>}
      {!rows && <>
        <div className="steps"><b>1. Where's the data?</b></div>
        <label className="small">Connection
          <select value={connection} onChange={e => pickConnection(e.target.value)}>
            <option value="">Choose a connection…</option>
            {connections.map(c => <option key={c.name} value={c.name}>{c.name} ({c.type})</option>)}
          </select>
        </label>
        {connections.length === 0 && <p className="muted small">No connections yet: add one on <Link to="/connections">Connections</Link>.</p>}
        {connection && <>
          <div className="steps"><b>2. Which table?</b> <span className="muted small">{tables ? `${tables.length} found` : 'listing…'}</span></div>
          <div className="table-pick">
            {(tables ?? []).map(t => <button key={t} className="link" disabled={busy} onClick={() => review(t)}>{t}</button>)}
          </div>
        </>}
        <div className="row"><button onClick={onCancel}>Cancel</button></div>
      </>}

      {rows && <>
        <div className="steps"><b>3. What's in it?</b> <span className="muted small">{connection} · {table}</span></div>
        <div className="row wrap">
          <label className="small grow">Name<input value={label} onChange={e => setLabel(e.target.value)} /></label>
          <label className="small">Id <span className="muted">(permanent)</span>
            <input className="mono" value={name} disabled={!!initial} onChange={e => setName(slug(e.target.value))} /></label>
        </div>
        {nameTaken && <div className="error small">The id {name} is taken.</div>}
        <p className="muted small"><b>Dimensions</b> are what people group and filter by (region, day); <b>measures</b> are the numbers
          they add up (revenue). A count of rows is always there.</p>
        <table className="fields-table">
          <thead><tr><th /><th>Column</th><th>Name shown</th><th>Use as</th><th>Type</th></tr></thead>
          <tbody>
            {rows.map((r, i) => (
              <tr key={r.column} className={r.include ? '' : 'off'}>
                <td><input type="checkbox" checked={r.include} onChange={e => set(i, { include: e.target.checked })} /></td>
                <td className="mono small">{r.column}</td>
                <td><input value={r.label} disabled={!r.include} onChange={e => set(i, { label: e.target.value })} /></td>
                <td>
                  <select value={r.role} disabled={!r.include} onChange={e => {
                    const role = e.target.value as FieldRow['role']
                    set(i, { role, type: role === 'measure' ? 'sum' : 'number' })
                  }}>
                    <option value="dimension">dimension</option><option value="measure">measure</option>
                  </select>
                </td>
                <td>
                  <select value={r.type} disabled={!r.include} onChange={e => set(i, { type: e.target.value })}>
                    {(r.role === 'measure' ? MEASURE_TYPES : DIMENSION_TYPES).map(t => <option key={t} value={t}>{t.replace('_', ' ')}</option>)}
                  </select>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
        {skipped.length > 0 && <p className="muted small">Left out (names Magnum BI can't use as fields yet): {skipped.join(', ')}.</p>}
        <div className="row">
          <button onClick={initial ? onCancel : () => setRows(null)}>{initial ? 'Cancel' : 'Back'}</button>
          <button className="primary" disabled={busy || !name || !label.trim() || nameTaken} onClick={save}>{busy ? 'Checking…' : 'Save model'}</button>
        </div>
      </>}
    </div>
  )
}

/** Org admins: calculations shared with everyone, as measures of their models (made here or in Explore). */
function SharedFormulas() {
  const [list, setList] = useState<SharedFormula[]>([])
  const [editing, setEditing] = useState<{ model: string; formula: Formula | null } | null>(null)
  const [pick, setPick] = useState('')
  const [error, setError] = useState<string | null>(null)
  const { models, reload } = useModels()
  const load = () => api.formulas().then(r => setList(r.formulas), e => setError(e.message))
  useEffect(() => { load() }, [])
  const usable = models.filter(m => !m.blend)
  const model = models.find(m => m.name === editing?.model)

  const save = async (f: Formula) => {
    await api.saveFormula(editing!.model, f)
    setEditing(null); forgetModels(); reload(); load()
  }
  const remove = async (s: SharedFormula) => {
    if (!confirm(`Delete the formula "${s.formula.label ?? s.formula.name}"? Dashboards using it stop working.`)) return
    try { await api.deleteFormula(s.model, s.formula.name); forgetModels(); reload(); load() } catch (e) { setError((e as Error).message) }
  }
  return <>
    <h2 className="section">Shared formulas</h2>
    <p className="muted small">Calculations on a model's measures, such as <span className="mono">revenue / transactions</span>, or
      fields of each row, such as <span className="mono">amount * qty</span>. Everyone in your organisation finds them among the
      model's fields. Anyone who explores can also add a formula to their own chart.</p>
    {error && <div className="error">{error}</div>}
    <div className="row">
      <select value={pick} onChange={e => setPick(e.target.value)} aria-label="Model">
        <option value="">Choose a model…</option>
        {usable.map(m => <option key={m.name} value={m.name}>{m.label}</option>)}
      </select>
      <button disabled={!pick} onClick={() => setEditing({ model: pick, formula: null })}>ƒ New formula</button>
    </div>
    {list.map(s => (
      <div key={`${s.model}.${s.formula.name}`} className="card person row wrap">
        <b>{s.formula.label ?? s.formula.name}</b>
        <span className="muted small mono">{models.find(m => m.name === s.model)?.label ?? s.model} · {s.formula.per_row ? 'per row' : 'on totals'} · {s.formula.expr}</span>
        {s.problem && <span className="error small">{s.problem}</span>}
        <span className="grow" />
        <span className="muted small">{s.updated_by ? `by ${s.updated_by}, ` : ''}{new Date(s.updated_at).toLocaleDateString()}</span>
        <button onClick={() => setEditing({ model: s.model, formula: s.formula })}>Edit</button>
        <button className="danger" onClick={() => remove(s)}>Delete</button>
      </div>
    ))}
    {editing && model && (
      <FormulaEditor model={model} initial={editing.formula} shared canShare
        taken={[...model.dimensions.map(d => d.name), ...model.measures.map(m => m.name)]}
        onSave={save} onCancel={() => setEditing(null)} />
    )}
  </>
}
