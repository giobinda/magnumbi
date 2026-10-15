import { useEffect, useState } from 'react'
import { useNavigate, useSearchParams } from 'react-router-dom'
import { api } from '../api'
import type { DashboardSummary, Tile } from '../api'
import Explorer from '../components/Explorer'
import { useModels } from '../hooks'

export default function ExplorePage() {
  const { models, loaded, error } = useModels()
  const [params] = useSearchParams()
  const navigate = useNavigate()
  const [pending, setPending] = useState<Tile | null>(null)
  const [dashboards, setDashboards] = useState<DashboardSummary[]>([])
  const [target, setTarget] = useState('')

  useEffect(() => { api.dashboards().then(r => { setDashboards(r.dashboards); setTarget(r.dashboards[0]?.id ?? '') }) }, [])

  const addTo = async () => {
    if (!pending || !target) return
    const { dashboard } = await api.dashboard(target)
    const bottom = Math.max(0, ...dashboard.tiles.map(t => t.layout.y + t.layout.h))
    let id = (pending.title || pending.query.measures?.[0] || 'tile').toLowerCase().replace(/[^a-z0-9]+/g, '_')
    while (dashboard.tiles.some(t => t.id === id)) id += '_2'
    dashboard.tiles.push({ ...pending, id, layout: { x: 0, y: bottom, w: pending.type === 'kpi' ? 3 : 6, h: pending.type === 'kpi' ? 2 : 5 } })
    await api.saveDashboard(target, dashboard)
    navigate(`/dashboards/${target}`)
  }

  const start = params.get('model')
  const ordered = start ? [...models].sort((a, b) => (a.name === start ? -1 : b.name === start ? 1 : 0)) : models
  return (
    <div className="page full">
      {error && <div className="error">{error}</div>}
      {loaded && <Explorer models={ordered} submitLabel="Add to dashboard…" onSubmit={setPending} />}
      {pending && (
        <div className="modal-bg" onMouseDown={e => { if (e.target === e.currentTarget) setPending(null) }}>
          <div className="modal">
            <h2>Add to dashboard</h2>
            <select value={target} onChange={e => setTarget(e.target.value)}>
              {dashboards.map(d => <option key={d.id} value={d.id}>{d.title}</option>)}
            </select>
            <div className="row"><button onClick={() => setPending(null)}>Cancel</button><button className="primary" disabled={!target} onClick={addTo}>Add</button></div>
          </div>
        </div>
      )}
    </div>
  )
}
