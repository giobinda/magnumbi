import { ask } from '../dialogs'
import { useEffect, useState } from 'react'
import { Link, useNavigate } from 'react-router-dom'
import { api } from '../api'
import type { ConnectionInfo, Formula, SharedFormula } from '../api'
import FormulaEditor from '../components/FormulaEditor'
import { useAuth } from '../auth'
import { forgetModels, useModels } from '../hooks'
import { DIMENSION_TYPES, MEASURE_TYPES, keepPaths, lookupsOf, modelFrom, rowsFor, suggestMatch, withLookups, withoutLookups } from '../modelEditor'
import type { FieldRow, LookupDef, MeasureDef, ModelDef } from '../modelEditor'
import { starterDashboard } from '../starter'

const slug = (s: string) => s.toLowerCase().replace(/[^a-z0-9_]+/g, '_').replace(/^_+|_+$/g, '').replace(/^(\d)/, 't_$1') || 'model'

/** Org admins: the organisation's own models, made from its connections' tables. System owners, in the
 *  platform workspace: the platform's, shared with every organisation its list of models allows. */
export default function ModelsPage() {
  const [own, setOwn] = useState<{ model: ModelDef; updated_at: string; updated_by: string | null }[]>([])
  const [editing, setEditing] = useState<ModelDef | 'new' | null>(null)
  const [error, setError] = useState<string | null>(null)
  const { models } = useModels()
  const navigate = useNavigate()
  const { auth } = useAuth()
  const inPlatform = !auth?.org
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
    if (!(await ask.confirm(`Delete the model "${m.label ?? m.name}"? Dashboards and blends using it stop working.`, { danger: true, ok: 'Delete' }))) return
    try { await api.deleteOrgModel(m.name); forgetModels(); load() } catch (e) { setError((e as Error).message) }
  }
  if (editing) {
    return <ModelWizard initial={editing === 'new' ? null : editing} taken={models.map(m => m.name)}
      onDone={() => { setEditing(null); forgetModels(); load() }} onCancel={() => setEditing(null)} />
  }
  const platform = models.filter(m => !m.own && !m.blend && !own.some(o => o.model.name === m.name))
  return (
    <div className="page narrow">
      <h1>Models</h1>
      <p className="muted">A model says what your data means: which table, the fields people group and filter by (dimensions), and
        the numbers they add up (measures). Make one from any table of your connections; {inPlatform
          ? <>it becomes one of the platform's models: every organisation allowed it (on <Link to="/organisations">Organisations</Link>) uses it on its
            own connection of the same name, with its own data, as for shared master data.</>
          : <>everyone in your organisation can then explore it and build dashboards on it.</>}</p>
      {error && <div className="error">{error}</div>}
      <div className="row"><button className="primary" data-tour="new-model" onClick={() => setEditing('new')}>+ New model</button></div>

      <h2 className="section">{inPlatform ? "The platform's models, made here" : "Your organisation's models"}</h2>
      {own.length === 0 && <p className="muted small">None yet: start with <b>New model</b>.</p>}
      {own.map(({ model: m, updated_at, updated_by }) => (
        <div key={m.name} className="card person row wrap">
          <b>{m.label ?? m.name}</b>
          <span className="muted small mono">{m.name} · {m.connection} · {m.table} · {m.dimensions.length} dimensions · {m.measures.length} measures</span>
          <span className="grow" />
          <span className="muted small">{updated_by ? `by ${updated_by}, ` : ''}{new Date(updated_at).toLocaleDateString()}</span>
          <Link className="btn" to={`/explore?model=${encodeURIComponent(m.name)}`}>Explore</Link>
          <button data-tour="starter" onClick={() => starter(m.name)}>Starter dashboard</button>
          <button onClick={() => setEditing(m)}>Edit</button>
          <button className="danger" onClick={() => remove(m)}>Delete</button>
        </div>
      ))}

      {platform.length > 0 && <>
        <h2 className="section">{inPlatform ? 'From the project files' : 'From the platform'}</h2>
        <p className="muted small">{inPlatform ? 'YAML files in project/models, changed through git.' : "Defined by your Magnum BI platform's team, for every organisation."}</p>
        {platform.map(m => (
          <div key={m.name} className="card person row wrap">
            <span>{m.label}</span><span className="muted small mono">{m.name}</span>
            <span className="grow" />
            <button data-tour="starter" onClick={() => starter(m.name)}>Starter dashboard</button>
          </div>
        ))}
      </>}
      <SharedFormulas />
      <p className="muted small">Restrictions on what members see apply to models by name, and follow the same table into any new model
        on it. A model on a different table or view is different data: check <Link to={inPlatform ? '/organisations' : '/members'}>{inPlatform ? 'Organisations' : 'Members'}</Link> after adding one.</p>
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
      setRows(rowsFor(p.model, initial ? withoutLookups(initial) : undefined))
      setCount(initial?.measures.find(m => m.type === 'count' && !m.sql) ?? p.model.measures[0])
    } catch (e) { setError((e as Error).message) } finally { setBusy(false) }
  }
  // editing: straight to the fields
  useEffect(() => { if (initial) review(initial.table ?? '') }, [])  // eslint-disable-line react-hooks/exhaustive-deps
  const save = async () => {
    if (!rows) return
    setError(null); setBusy(true)
    const whole = withLookups(modelFrom({ name, label, connection, table }, rows, count), lookups)
    const m = { ...whole, hierarchies: keepPaths(paths, whole.dimensions.map(d => d.name)) }
    try { await api.saveOrgModel(m, true); await api.saveOrgModel(m); onDone() } catch (e) { setError((e as Error).message) } finally { setBusy(false) }
  }
  const [paths, setPaths] = useState<Record<string, string[]>>(initial?.hierarchies ?? {})
  const [lookups, setLookups] = useState<LookupDef[]>(initial ? lookupsOf(initial) : [])
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
        <DrillPaths paths={paths} onChange={setPaths}
          dims={withLookups(modelFrom({ name, label, connection, table }, rows, count), lookups).dimensions
            .map(d => ({ name: d.name, label: d.label || d.name }))} />
        <Lookups connection={connection} table={table} mine={rows.map(r => r.column)} lookups={lookups} onChange={setLookups} />
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
    if (!(await ask.confirm(`Delete the formula "${s.formula.label ?? s.formula.name}"? Dashboards using it stop working.`, { danger: true, ok: 'Delete' }))) return
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

/** The model's drill paths: clicking a bar goes one step down (Region, then its stores). Each a name
 *  and its dimensions in order; one with fewer than two steps isn't saved. */
function DrillPaths({ paths, onChange, dims }: {
  paths: Record<string, string[]>; onChange: (p: Record<string, string[]>) => void; dims: { name: string; label: string }[]
}) {
  const labelOf = (d: string) => dims.find(x => x.name === d)?.label ?? d
  const entries = Object.entries(paths)
  const put = (i: number, name: string, steps: string[]) =>
    onChange(Object.fromEntries(entries.map(([h, p], j) => (j === i ? [name, steps] : [h, p]))))
  const add = () => {
    let n = 1
    while (paths[`path_${n}`]) n++
    onChange({ ...paths, [`path_${n}`]: [] })
  }
  return <>
    <div className="steps"><b>4. Drill paths</b> <span className="muted small">optional</span></div>
    <p className="muted small">Clicking a bar or a slice goes one step down a path: a region, then its stores. A tile drills
      along the path chosen in its editor (Drill).</p>
    {entries.map(([h, steps], i) => (
      <div key={i} className="drill-path row wrap">
        <input className="mono small" aria-label="Path name" value={h} onChange={e => put(i, slug(e.target.value), steps)} />
        <span className="drill-steps">
          {steps.map((d, k) => (
            <span key={d} className="chip dimension">
              {k > 0 && <span className="muted">› </span>}{labelOf(d)}
              <button className="link" title={`Take ${labelOf(d)} out`} onClick={() => put(i, h, steps.filter(x => x !== d))}>×</button>
            </span>
          ))}
        </span>
        <select value="" aria-label="Next step" onChange={e => e.target.value && put(i, h, [...steps, e.target.value])}>
          <option value="">{steps.length ? '+ next step' : '+ first step'}</option>
          {dims.filter(d => !steps.includes(d.name)).map(d => <option key={d.name} value={d.name}>{d.label}</option>)}
        </select>
        {steps.length < 2 && <span className="muted small">two steps or more</span>}
        <button className="link danger" onClick={() => onChange(Object.fromEntries(entries.filter((_, j) => j !== i)))}>Remove</button>
      </div>
    ))}
    <div className="row"><button onClick={add} disabled={dims.length < 2}>+ New drill path</button></div>
  </>
}

/** Lookups: other tables of the connection with one row per key (customers for orders), whose
 *  columns become the model's dimensions (a customer's country). The main table's numbers count
 *  once: a lookup with several rows per key is refused when the model is saved. */
function Lookups({ connection, table, mine, lookups, onChange }: {
  connection: string; table: string; mine: string[]; lookups: LookupDef[]; onChange: (l: LookupDef[]) => void
}) {
  const [tables, setTables] = useState<string[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [columns, setColumns] = useState<Record<string, { column: string; type: string; label: string }[]>>({})
  // a table's columns (from the proposal Models › New model makes), once
  const columnsOf = async (t: string) => {
    if (columns[t]) return columns[t]
    const p = await api.proposeModel(connection, t, 'lookup')
    const cols = [
      ...p.model.dimensions.map(d => ({ column: d.name, type: d.type, label: d.label ?? d.name })),
      ...p.model.measures.filter(m => m.sql).map(m => ({ column: (m.sql ?? '').replace(/["`]/g, ''), type: 'number', label: m.label ?? m.name })),
    ]
    setColumns(c => ({ ...c, [t]: cols }))
    return cols
  }
  // editing: every column of each lookup, the saved ones ticked
  useEffect(() => {
    lookups.forEach(l => { columnsOf(l.table).catch(e => setError((e as Error).message)) })
  }, [])  // eslint-disable-line react-hooks/exhaustive-deps
  const put = (i: number, l: LookupDef) => onChange(lookups.map((x, j) => (j === i ? l : x)))
  const add = async (t: string) => {
    setError(null)
    try {
      const cols = await columnsOf(t)
      let as = slug(t.split('.').pop() ?? t)
      while (lookups.some(l => l.as === as) || as === 'base') as = `${as}_2`
      const match = suggestMatch(mine, cols.map(c => c.column), as)
      onChange([...lookups, { table: t, as, on: [match], fields: cols.map(c => ({ ...c, include: false })) }])
    } catch (e) { setError((e as Error).message) }
  }
  const fieldsOf = (l: LookupDef) => {
    const known = columns[l.table] ?? []
    return [...l.fields, ...known.filter(c => !l.fields.some(f => f.column === c.column)).map(c => ({ ...c, include: false }))]
  }
  return <>
    <div className="steps"><b>5. Lookups</b> <span className="muted small">optional</span></div>
    <p className="muted small">Another table with one row per key, matched to this one: customers for orders, products for sales.
      Its columns become this model's fields (a customer's country), while the numbers stay this table's and count once.
      Restrictions on that table apply here too.</p>
    {lookups.map((l, i) => (
      <fieldset key={i} className="join lookup">
        <legend>{l.table} <span className="muted small">as {l.as}</span></legend>
        {l.on.map((pair, k) => (
          <div key={k} className="row key">
            <span className="small">{table}</span>
            <select value={pair[0]} onChange={e => put(i, { ...l, on: l.on.map((p, n) => (n === k ? [e.target.value, p[1]] : p)) })}>
              <option value="">column…</option>{mine.map(c => <option key={c} value={c}>{c}</option>)}
            </select>
            <span>=</span>
            <span className="small">{l.table}</span>
            <select value={pair[1]} onChange={e => put(i, { ...l, on: l.on.map((p, n) => (n === k ? [p[0], e.target.value] : p)) })}>
              <option value="">column…</option>{(columns[l.table] ?? []).map(c => <option key={c.column} value={c.column}>{c.column}</option>)}
            </select>
            {l.on.length > 1 && <button className="link danger" onClick={() => put(i, { ...l, on: l.on.filter((_, n) => n !== k) })}>×</button>}
          </div>
        ))}
        <button className="link small" onClick={() => put(i, { ...l, on: [...l.on, ['', '']] })}>+ another column to match</button>
        <table className="fields-table">
          <thead><tr><th /><th>Its column</th><th>Name shown</th></tr></thead>
          <tbody>{fieldsOf(l).map(f => (
            <tr key={f.column} className={f.include ? '' : 'off'}>
              <td><input type="checkbox" checked={f.include} onChange={e => put(i, { ...l, fields: fieldsOf(l).map(x => (x.column === f.column ? { ...x, include: e.target.checked } : x)) })} /></td>
              <td className="mono small">{f.column}</td>
              <td><input value={f.label} disabled={!f.include} onChange={e => put(i, { ...l, fields: fieldsOf(l).map(x => (x.column === f.column ? { ...x, label: e.target.value } : x)) })} /></td>
            </tr>
          ))}</tbody>
        </table>
        <button className="link danger" onClick={() => onChange(lookups.filter((_, j) => j !== i))}>Remove this lookup</button>
      </fieldset>
    ))}
    {error && <div className="error small">{error}</div>}
    <div className="row lookup-add">
      {tables === null
        ? <button onClick={() => api.tables(connection).then(r => setTables(r.tables.filter(t => t !== table)), e => setError(e.message))}>+ Add a lookup</button>
        : <select value="" onChange={e => e.target.value && add(e.target.value)}>
            <option value="">Look up which table?</option>
            {tables.map(t => <option key={t} value={t}>{t}</option>)}
          </select>}
    </div>
  </>
}
