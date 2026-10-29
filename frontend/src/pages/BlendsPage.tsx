import { ask } from '../dialogs'
import { blendName, matchSentence, suggestKeys } from '../blendForm'
import type { Key } from '../blendForm'
import { useEffect, useState } from 'react'
import { Link } from 'react-router-dom'
import { api, isTemporal } from '../api'
import type { Blend, BlendJoin, ModelInfo } from '../api'
import { useAuth, useCan } from '../auth'
import { forgetModels, useModels } from '../hooks'

const GRAINS = ['hour', 'day', 'week', 'month', 'quarter', 'year']
const CACHE: [string, number | null][] = [['Live data', null], ['15 minutes', 900], ['1 hour', 3600], ['1 day', 86400]]


const parseKey = (s: string): Key => {
  const [a = '', b = ''] = s.split('=').map(x => x.trim())
  const [base, grain = ''] = a.split(':')
  return { base, other: b.split(':')[0], grain }
}
const keyText = (k: Key) => (k.grain ? `${k.base}:${k.grain} = ${k.other}:${k.grain}` : `${k.base} = ${k.other}`)

function describe(b: Blend, models: ModelInfo[]) {
  const label = (n: string) => models.find(m => m.name === n)?.label ?? n
  return b.joins.map(j => `${label(b.base)} + ${label(j.model)} on ${j.on.join(', ')}`).join('; ')
}

/** Blends: models joined on shared fields, across connections and engines. Each model is
 *  summarised on its own connection, then the results are joined on the organisation's ScramDB. */
export default function BlendsPage() {
  const { auth } = useAuth()
  const can = useCan()
  const { models: all } = useModels()
  const [list, setList] = useState<Blend[] | null>(null)
  const [editing, setEditing] = useState<Blend | 'new' | null>(null)
  const [error, setError] = useState<string | null>(null)
  const load = () => api.blends().then(r => setList(r.blends), e => setError(e.message))
  useEffect(() => { load() }, [])
  const models = all.filter(m => !m.blend)

  const remove = async (b: Blend) => {
    if (!(await ask.confirm(`Delete the blend "${b.label || b.name}"? Tiles that use it stop working.`, { danger: true, ok: 'Delete' }))) return
    try { await api.deleteBlend(b.name); forgetModels(); load() } catch (e) { setError((e as Error).message) }
  }

  return (
    <div className="page narrow">
      <h1>Blends <span className="muted">· {auth?.org?.name ?? 'platform workspace'}</span></h1>
      <p className="muted">A blend joins models on fields they share, even when their data lives in different databases
        (e.g. trips in DuckDB and weather in PostgreSQL). Each model is summarised on its own connection at the join keys,
        then the summaries are joined on this organisation's ScramDB. Use a blend in Explore like any model; its fields
        are named <code>model.field</code>.</p>
      {error && <div className="error">{error}</div>}
      {list && (
        <table className="data">
          <thead><tr><th>Blend</th><th>Joins</th><th>Defined</th><th /></tr></thead>
          <tbody>{list.map(b => (
            <tr key={b.name}>
              <td><b>{b.label || b.name}</b>{b.label && <div className="muted small mono">{b.name}</div>}</td>
              <td className="small">{describe(b, all)}</td>
              <td className="small">{b.editable ? 'here' : 'in the project (code)'}</td>
              <td className="nowrap">
                {can('explore') && <Link className="link" to={`/explore?model=${encodeURIComponent(b.name)}`}>Explore</Link>}
                {b.editable && can('edit') && <> <button className="link" onClick={() => setEditing(b)}>Edit</button>{' '}
                  <button className="link danger" onClick={() => remove(b)}>Delete</button></>}
              </td>
            </tr>
          ))}</tbody>
        </table>
      )}
      {list?.length === 0 && <p className="muted small">No blends yet.</p>}
      {can('edit') && !editing && <div className="row"><button className="primary" disabled={models.length < 2}
        onClick={() => setEditing('new')}>+ New blend</button></div>}
      {editing && <BlendForm initial={editing === 'new' ? null : editing} models={models}
        onDone={() => { setEditing(null); forgetModels(); load() }} onCancel={() => setEditing(null)} />}
    </div>
  )
}

function BlendForm({ initial, models, onDone, onCancel }: {
  initial: Blend | null; models: ModelInfo[]; onDone: () => void; onCancel: () => void
}) {
  const [name, setName] = useState(initial?.name ?? '')
  const [label, setLabel] = useState(initial?.label ?? '')
  const [description, setDescription] = useState(initial?.description ?? '')
  const [base, setBase] = useState(initial?.base ?? models[0]?.name ?? '')
  const [joins, setJoins] = useState<{ model: string; keys: Key[] }[]>(
    initial?.joins.map(j => ({ model: j.model, keys: j.on.map(parseKey) })) ?? [{ model: '', keys: [{ base: '', other: '', grain: '' }] }])
  const [cache, setCache] = useState<number | null>(initial?.cache ?? 3600)
  const [msg, setMsg] = useState<string | null>(null)
  const [checked, setChecked] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  // the id follows the label until it's typed (a new blend; an id never changes after)
  const [nameTyped, setNameTyped] = useState(Boolean(initial))

  const model = (n: string) => models.find(m => m.name === n)
  const dims = (n: string) => model(n)?.dimensions ?? []
  const temporal = (n: string, f: string) => isTemporal(dims(n).find(d => d.name === f)?.type)
  const setJoin = (i: number, j: { model: string; keys: Key[] }) => setJoins(joins.map((x, k) => (k === i ? j : x)))

  const def = (): Blend => ({
    name: name.trim(), label: label.trim() || null, description: description.trim() || null, base, cache,
    joins: joins.map((j): BlendJoin => ({ model: j.model, on: j.keys.filter(k => k.base && k.other).map(keyText) })),
  })
  const save = async () => {
    setBusy(true); setMsg(null)
    try { await api.saveBlend(def()); onDone() } catch (e) { setMsg((e as Error).message) } finally { setBusy(false) }
  }
  // a dry run: everything a save checks, nothing saved
  const check = async () => {
    setBusy(true); setMsg(null); setChecked(null)
    try { await api.saveBlend(def(), true); setChecked('Checked: it can be saved.') } catch (e) { setMsg((e as Error).message) } finally { setBusy(false) }
  }
  const fieldLabel = (m: string) => (f: string) => dims(m).find(d => d.name === f)?.label || f
  // a model added: its fields to match, proposed
  const pick = (i: number, m: string) => setJoin(i, { model: m, keys: m ? suggestKeys(dims(base), dims(m)) : [{ base: '', other: '', grain: '' }] })

  return (
    <form className="card org-card connection-form blend-form" onSubmit={e => { e.preventDefault(); save() }}>
      <h2>{initial ? `Edit ${initial.label || initial.name}` : 'New blend'}</h2>
      <div className="grid2">
        <label className="small">Name
          <input value={label} onChange={e => { setLabel(e.target.value); if (!nameTyped) setName(blendName(e.target.value)) }} placeholder="Taxi trips × weather" />
        </label>
        <label className="small">Id <span className="muted">(permanent: letters, digits, _)</span>
          <input className="mono" value={name} disabled={!!initial} required pattern="[A-Za-z_][A-Za-z0-9_]*"
            onChange={e => { setName(e.target.value); setNameTyped(true) }} placeholder="taxi_weather" />
        </label>
      </div>
      <label className="small">Description
        <input value={description} onChange={e => setDescription(e.target.value)} />
      </label>
      <label className="small">Start from <span className="muted">· the model whose rows the others are added to</span>
        <select value={base} onChange={e => setBase(e.target.value)}>
          {models.map(m => <option key={m.name} value={m.name}>{m.label}</option>)}
        </select>
      </label>

      {joins.map((j, i) => (
        <fieldset key={i} className="join">
          <legend>Add the data of</legend>
          <div className="row">
            <select value={j.model} onChange={e => pick(i, e.target.value)}>
              <option value="">Choose a model…</option>
              {models.filter(m => m.name !== base && (m.name === j.model || !joins.some(x => x.model === m.name)))
                .map(m => <option key={m.name} value={m.name}>{m.label}</option>)}
            </select>
            {joins.length > 1 && <button type="button" className="link danger" onClick={() => setJoins(joins.filter((_, k) => k !== i))}>Remove</button>}
          </div>
          {j.model && j.keys.map((k, n) => (
            <div key={n} className="row key">
              <select value={k.base} onChange={e => setJoin(i, { ...j, keys: j.keys.map((x, m) => (m === n ? { ...x, base: e.target.value } : x)) })}>
                <option value="">{model(base)?.label} field…</option>
                {dims(base).map(d => <option key={d.name} value={d.name}>{d.label || d.name}</option>)}
              </select>
              <span>=</span>
              <select value={k.other} onChange={e => setJoin(i, { ...j, keys: j.keys.map((x, m) => (m === n ? { ...x, other: e.target.value } : x)) })}>
                <option value="">{model(j.model)?.label} field…</option>
                {dims(j.model).map(d => <option key={d.name} value={d.name}>{d.label || d.name}</option>)}
              </select>
              {temporal(base, k.base) && temporal(j.model, k.other) && (
                <select value={k.grain} title="Match dates and times by" onChange={e => setJoin(i, { ...j, keys: j.keys.map((x, m) => (m === n ? { ...x, grain: e.target.value } : x)) })}>
                  <option value="">the exact moment</option>
                  {GRAINS.map(g => <option key={g} value={g}>the same {g}</option>)}
                </select>
              )}
              {j.keys.length > 1 && <button type="button" className="link danger" onClick={() => setJoin(i, { ...j, keys: j.keys.filter((_, m) => m !== n) })}>×</button>}
            </div>
          ))}
          {j.model && <button type="button" className="link" onClick={() => setJoin(i, { ...j, keys: [...j.keys, { base: '', other: '', grain: '' }] })}>+ another field to match</button>}
          {j.model && <p className="muted small">{matchSentence(model(base)?.label ?? base, model(j.model)?.label ?? j.model, j.keys,
            { base: fieldLabel(base), other: fieldLabel(j.model) })}</p>}
        </fieldset>
      ))}
      <button type="button" className="link" onClick={() => setJoins([...joins, { model: '', keys: [{ base: '', other: '', grain: '' }] }])}>+ add another model's data</button>

      <label className="small">Reuse results for
        <select value={cache ?? ''} onChange={e => setCache(e.target.value ? Number(e.target.value) : null)}>
          {CACHE.map(([l, v]) => <option key={l} value={v ?? ''}>{l}</option>)}
        </select>
      </label>
      <p className="muted small">Measures that add up (counts, sums, minimums, maximums, averages) can be combined across the
        models; distinct counts and custom formulas stay available on each model alone. When a model has several rows per
        join key for the fields you pick, the other models' measures would be counted twice: such queries are refused.</p>
      {msg && <div className="error">{msg}</div>}
      {checked && !msg && <div className="ok">{checked}</div>}
      <div className="row">
        <button type="button" onClick={onCancel}>Cancel</button>
        <button type="button" disabled={busy || !name || !base || joins.some(j => !j.model)} onClick={check}>Check</button>
        <button className="primary" disabled={busy || !name || !base || joins.some(j => !j.model)}>{busy ? 'Saving…' : 'Save'}</button>
      </div>
    </form>
  )
}
