import { useEffect, useState } from 'react'
import { useNavigate, useSearchParams } from 'react-router-dom'
import { api } from '../api'
import type { DashboardSummary, ChartTile } from '../api'
import Explorer from '../components/Explorer'
import { addTile } from '../pin'
import { useModels } from '../hooks'

export default function ExplorePage() {
  const { models, loaded, error, reload } = useModels()
  const [params] = useSearchParams()
  const navigate = useNavigate()
  const [pending, setPending] = useState<ChartTile | null>(null)
  const [dashboards, setDashboards] = useState<DashboardSummary[]>([])
  const [target, setTarget] = useState('')

  useEffect(() => { api.dashboards().then(r => { setDashboards(r.dashboards); setTarget(r.dashboards[0]?.id ?? '') }) }, [])

  const addTo = async () => {
    if (!pending || !target) return
    await addTile(target, pending)
    navigate(`/dashboards/${target}`)
  }

  const start = params.get('model')
  const ordered = start ? [...models].sort((a, b) => (a.name === start ? -1 : b.name === start ? 1 : 0)) : models
  return (
    <div className="page full">
      {error && <div className="error">{error}</div>}
      {loaded && <Explorer models={ordered} submitLabel="Add to dashboard…" onSubmit={setPending} onModelsChanged={reload} />}
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
