import { useEffect, useState } from 'react'
import { Link, useNavigate } from 'react-router-dom'
import { api } from '../api'
import type { DashboardSummary } from '../api'
import { useModels } from '../hooks'
import { useCan } from '../auth'
import GetStarted from '../components/GetStarted'

export default function Home() {
  const [list, setList] = useState<DashboardSummary[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const { models, modelErrors } = useModels()
  const navigate = useNavigate()
  const can = useCan()

  useEffect(() => { api.dashboards().then(r => setList(r.dashboards), e => setError(e.message)) }, [])

  const create = async () => {
    const title = prompt('Dashboard title')
    if (!title) return
    const base = title.toLowerCase().replace(/[^a-z0-9]+/g, '_').replace(/^_|_$/g, '') || 'dashboard'
    let id = base, n = 2
    while (list?.some(d => d.id === id)) id = `${base}_${n++}`
    try {
      await api.saveDashboard(id, { title, filters: [], tiles: [] })
      navigate(`/dashboards/${id}?edit`)
    } catch (e) { alert((e as Error).message) }
  }

  return (
    <div className="page">
      <div className="dash-head">
        <h1>Dashboards</h1>
        <div className="dash-actions">
          {can('create') && <button className="primary" onClick={create}>+ New dashboard</button>}
        </div>
      </div>
      <GetStarted />
      {error && <div className="error">{error}</div>}
      {Object.entries(modelErrors).map(([f, e]) => <div key={f} className="warn">⚠ {f}: {e}</div>)}
      <div className="cards">
        {list?.map(d => (
          <Link key={d.id} to={`/dashboards/${d.id}`} className="card">
            <div className="card-title">{d.title}</div>
            {d.error ? <div className="error small">{d.error}</div> : <div className="muted small">{d.description || `${d.tiles} tiles`}</div>}
            <div className="muted small mono">{d.id}</div>
          </Link>
        ))}
        {list?.length === 0 && <div className="empty">No dashboards yet.</div>}
      </div>
      {can('explore') && <h2 className="section">Models</h2>}
      <div className="cards">
        {can('explore') && models.map(m => (
          <Link key={m.name} to={`/explore?model=${m.name}`} className="card">
            <div className="card-title">{m.label}</div>
            <div className="muted small">{m.description}</div>
            <div className="muted small">{m.dimensions.length} dimensions · {m.measures.length} measures · {m.connection_type ?? `needs a "${m.connection}" connection`}</div>
          </Link>
        ))}
      </div>
    </div>
  )
}
