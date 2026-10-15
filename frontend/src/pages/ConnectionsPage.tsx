import { useEffect, useState } from 'react'
import type { FormEvent } from 'react'
import { api } from '../api'
import type { ConnectionInfo, ConnectionType } from '../api'
import { useAuth } from '../auth'
import { useModels } from '../hooks'

type Field = { key: string; label: string; kind?: 'text' | 'number' | 'bool' | 'tables' | 'gib'; optional?: boolean; placeholder?: string }

const SQL: Field[] = [
  { key: 'host', label: 'Host', placeholder: 'db.example.com' },
  { key: 'port', label: 'Port', kind: 'number', placeholder: '5432' },
  { key: 'database', label: 'Database' },
  { key: 'user', label: 'User' },
  { key: 'schema', label: 'Schema (where the tables are)', optional: true, placeholder: 'default' },
  { key: 'tls', label: 'TLS (verified certificate)', kind: 'bool' },
]
const FIELDS: Record<ConnectionType, Field[]> = {
  scramdb: SQL,
  postgres: SQL,
  bigquery: [
    { key: 'project', label: 'Project (where the data is)', placeholder: 'acme-analytics' },
    { key: 'dataset', label: 'Dataset (for unqualified tables)', optional: true },
    { key: 'billing_project', label: 'Billing project', optional: true, placeholder: 'same as project' },
    { key: 'location', label: 'Location', optional: true, placeholder: 'EU, US, europe-west1…' },
    { key: 'max_bytes_billed', label: 'Max data scanned per query (GiB)', kind: 'gib' },
  ],
  duckdb: [
    { key: 'database', label: 'Database file (.duckdb, in the organisation\'s directory)', optional: true, placeholder: 'warehouse.duckdb' },
    { key: 'tables', label: 'Tables: one per line, name = files (.parquet, .csv, .json, .avro) or iceberg:<table folder>', kind: 'tables', optional: true,
      placeholder: 'sample_sales = sales/*.parquet\nstores = s3://acme-data/stores.csv\norders = iceberg:gs://acme-lake/retail/orders' },
    { key: 's3_region', label: 'S3 region', optional: true, placeholder: 'eu-west-1' },
    { key: 's3_endpoint', label: 'S3-compatible endpoint', optional: true, placeholder: 'minio.example.com:9000' },
  ],
}
const LABELS: Record<ConnectionType, string> = { scramdb: 'ScramDB', postgres: 'PostgreSQL', bigquery: 'BigQuery', duckdb: 'DuckDB' }
const GIB = 1024 ** 3

type Values = Record<string, string | boolean>

function defaults(type: ConnectionType): Values {
  if (type === 'bigquery') return { max_bytes_billed: '10' }
  if (type === 'duckdb') return {}
  return { port: '5432', tls: true }
}

function fromConfig(c: ConnectionInfo): Values {
  const v: Values = {}
  for (const f of FIELDS[c.type]) {
    const x = c.config[f.key]
    if (x === null || x === undefined) continue
    v[f.key] = f.kind === 'bool' ? Boolean(x)
      : f.kind === 'gib' ? String(Number(x) / GIB)
      : f.kind === 'tables' ? Object.entries(x as Record<string, string>).map(([k, l]) => `${k} = ${l}`).join('\n')
      : String(x)
  }
  return v
}

function toConfig(type: ConnectionType, v: Values): Record<string, unknown> {
  const out: Record<string, unknown> = { type }
  for (const f of FIELDS[type]) {
    const x = v[f.key]
    if (f.kind === 'bool') { out[f.key] = Boolean(x); continue }
    const s = typeof x === 'string' ? x.trim() : ''
    if (!s) continue
    if (f.kind === 'number') out[f.key] = Number(s)
    else if (f.kind === 'gib') out[f.key] = Math.round(Number(s) * GIB)
    else if (f.kind === 'tables') {
      out[f.key] = Object.fromEntries(s.split('\n').map(l => l.split('=').map(p => p.trim())).filter(p => p.length === 2 && p[0] && p[1]))
    } else out[f.key] = s
  }
  return out
}

/** Org admins and system owners: the organisation's data connections. Models name a connection
 *  (e.g. `warehouse`); this organisation decides where that is and with which credentials. */
export default function ConnectionsPage() {
  const { auth } = useAuth()
  const { models } = useModels()
  const [list, setList] = useState<ConnectionInfo[] | null>(null)
  const [keySet, setKeySet] = useState(true)
  const [editing, setEditing] = useState<ConnectionInfo | 'new' | null>(null)
  const [error, setError] = useState<string | null>(null)
  const load = () => api.connections().then(r => { setList(r.connections); setKeySet(r.secret_key) }, e => setError(e.message))
  useEffect(() => { load() }, [])

  const needed = [...new Set(models.map(m => m.connection))].filter(n => !list?.some(c => c.name === n))
  const remove = async (c: ConnectionInfo) => {
    if (!confirm(`Delete the connection "${c.name}"? Models that use it stop working here.`)) return
    try { await api.deleteConnection(c.name); load() } catch (e) { setError((e as Error).message) }
  }

  return (
    <div className="page narrow">
      <h1>Connections <span className="muted">· {auth?.org?.name ?? 'platform workspace'}</span></h1>
      <p className="muted">Where this organisation's data is. Models name a connection; each organisation sets up its own, with its
        own credentials. Passwords and keys are stored encrypted and never shown again.</p>
      {!keySet && <div className="warn">MAGNUM_SECRET_KEY isn't set on the server: connections that need a password or key can't be saved.</div>}
      {error && <div className="error">{error}</div>}
      {needed.length > 0 && <div className="warn">Models here read from {needed.map(n => <code key={n}>{n}</code>).reduce<React.ReactNode[]>((a, x, i) => (i ? [...a, ', ', x] : [x]), [])}, which
        this organisation hasn't set up yet.</div>}

      {list && (
        <table className="data">
          <thead><tr><th>Name</th><th>Type</th><th>Where</th><th>Secret</th><th>Updated</th><th /></tr></thead>
          <tbody>{list.map(c => (
            <tr key={c.name}>
              <td><b>{c.name}</b></td><td>{LABELS[c.type]}</td>
              <td className="small mono">{where(c)}</td>
              <td className="small">{c.has_secret ? 'set' : '–'}</td>
              <td className="small">{new Date(c.updated_at).toLocaleDateString()}{c.updated_by ? ` · ${c.updated_by}` : ''}</td>
              <td><button className="link" onClick={() => setEditing(c)}>Edit</button> <button className="link danger" onClick={() => remove(c)}>Delete</button></td>
            </tr>
          ))}</tbody>
        </table>
      )}
      {list?.length === 0 && <p className="muted small">No connections yet.</p>}
      {!editing && <div className="row"><button className="primary" onClick={() => setEditing('new')}>+ Add connection</button></div>}
      {editing && <ConnectionForm initial={editing === 'new' ? null : editing} suggested={needed[0]}
        onDone={() => { setEditing(null); load() }} onCancel={() => setEditing(null)} />}
    </div>
  )
}

function where(c: ConnectionInfo): string {
  const x = c.config as Record<string, string>
  if (c.type === 'bigquery') return `${x.project}${x.dataset ? `.${x.dataset}` : ''}`
  if (c.type === 'duckdb') return [x.database, ...Object.keys((c.config.tables as object) ?? {})].filter(Boolean).join(', ')
  return `${x.host}:${x.port}/${x.database}${x.schema ? ` (${x.schema})` : ''}`
}

function ConnectionForm({ initial, suggested, onDone, onCancel }: {
  initial: ConnectionInfo | null; suggested?: string; onDone: () => void; onCancel: () => void
}) {
  const [name, setName] = useState(initial?.name ?? suggested ?? '')
  const [type, setType] = useState<ConnectionType>(initial?.type ?? 'postgres')
  const [values, setValues] = useState<Values>(initial ? fromConfig(initial) : defaults('postgres'))
  const [secret, setSecret] = useState('')
  const [keyId, setKeyId] = useState('')
  const [busy, setBusy] = useState(false)
  const [msg, setMsg] = useState<{ kind: 'ok' | 'error'; text: string } | null>(null)

  const secretValue = type === 'duckdb' ? (keyId || secret ? JSON.stringify({ key_id: keyId, secret }) : '') : secret
  const run = async (action: 'test' | 'save', e?: FormEvent) => {
    e?.preventDefault()
    setBusy(true); setMsg(null)
    const config = toConfig(type, values)
    try {
      if (action === 'test') setMsg({ kind: 'ok', text: (await api.testConnection(name, config, secretValue)).message })
      else { await api.saveConnection(name, config, secretValue); onDone() }
    } catch (x) { setMsg({ kind: 'error', text: (x as Error).message }) }
    setBusy(false)
  }
  const changeType = (t: ConnectionType) => { setType(t); setValues(defaults(t)); setSecret(''); setKeyId('') }
  const keep = initial?.has_secret && initial.type === type

  return (
    <form className="card org-card connection-form" onSubmit={e => run('save', e)}>
      <h2>{initial ? `Edit ${initial.name}` : 'New connection'}</h2>
      <label className="small">Name (what models refer to)
        <input value={name} disabled={!!initial} onChange={e => setName(e.target.value)} placeholder="warehouse" required />
      </label>
      <label className="small">Type
        <select value={type} disabled={!!initial} onChange={e => changeType(e.target.value as ConnectionType)}>
          {(Object.keys(LABELS) as ConnectionType[]).map(t => <option key={t} value={t}>{LABELS[t]}</option>)}
        </select>
      </label>
      {FIELDS[type].map(f => (
        <label key={f.key} className={f.kind === 'bool' ? 'check small' : 'small'}>
          {f.kind === 'bool'
            ? <><input type="checkbox" checked={Boolean(values[f.key])} onChange={e => setValues({ ...values, [f.key]: e.target.checked })} /> {f.label}</>
            : <>{f.label}{f.optional && <span className="muted"> (optional)</span>}
              {f.kind === 'tables'
                ? <textarea className="mono" rows={4} value={String(values[f.key] ?? '')} placeholder={f.placeholder}
                    onChange={e => setValues({ ...values, [f.key]: e.target.value })} />
                : <input type={f.kind === 'number' || f.kind === 'gib' ? 'number' : 'text'} value={String(values[f.key] ?? '')}
                    placeholder={f.placeholder} required={!f.optional} onChange={e => setValues({ ...values, [f.key]: e.target.value })} />}
            </>}
        </label>
      ))}
      {type === 'bigquery' && (
        <label className="small">Service account key (JSON){keep && <span className="muted"> · leave empty to keep the stored key</span>}
          <textarea className="mono" rows={4} autoComplete="off" spellCheck={false} value={secret} onChange={e => setSecret(e.target.value)}
            placeholder={'{"type": "service_account", ...}'} required={!keep} />
        </label>
      )}
      {type === 'duckdb' && <>
        <label className="small">Object storage key id (S3, or GCS HMAC) <span className="muted">(optional{keep ? '; empty keeps the stored keys' : ''})</span>
          <input autoComplete="off" value={keyId} onChange={e => setKeyId(e.target.value)} />
        </label>
        <label className="small">Object storage secret
          <input type="password" autoComplete="off" value={secret} onChange={e => setSecret(e.target.value)} />
        </label>
      </>}
      {(type === 'scramdb' || type === 'postgres') && (
        <label className="small">Password{keep && <span className="muted"> · leave empty to keep the stored password</span>}
          <input type="password" autoComplete="new-password" value={secret} onChange={e => setSecret(e.target.value)} />
        </label>
      )}
      {type === 'duckdb' && <p className="muted small">Files are read from this organisation's own directory on the server, or from its
        object storage. Each query runs in a locked sandbox that can only reach those places.</p>}
      {msg && <div className={msg.kind === 'error' ? 'error' : 'ok'}>{msg.text}</div>}
      <div className="row">
        <button type="button" onClick={onCancel}>Cancel</button>
        <button type="button" disabled={busy || !name} onClick={() => run('test')}>{busy ? 'Working…' : 'Test'}</button>
        <button className="primary" disabled={busy || !name}>Save</button>
      </div>
    </form>
  )
}
