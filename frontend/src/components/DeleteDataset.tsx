import { useEffect, useState } from 'react'
import { api } from '../api'
import type { ModelInfo } from '../api'
import { deleteSteps, namesToCheck, usesOf } from '../deleteUses'
import type { Item, Step, Target, Uses } from '../deleteUses'

const run = (s: Step) => ({
  dashboard: api.deleteDashboard, blend: api.deleteBlend, model: api.deleteOrgModel, connection: api.deleteConnection,
})[s.kind](s.name)

/** Deleting a connection or a model: says what uses it, and deletes with it only what's ticked. */
export default function DeleteDataset({ kind, name, label, note, models, onDone, onCancel }: {
  kind: Target['kind']; name: string; label: string
  /** what else goes with it (a file's uploaded copy) */
  note?: string
  models: ModelInfo[]
  onDone: () => void
  onCancel: () => void
}) {
  const [uses, setUses] = useState<Uses | null>(null)
  const [ticked, setTicked] = useState<Set<string>>(new Set())
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    const target: Target = { kind, name }
    let stale = false
    // each read on its own: what can't be read is said, and doesn't stop the deletion
    const names = namesToCheck(target, models)
    Promise.all([
      Promise.all(names.map(n => api.lineage(n).catch(() => null))),
      api.orgModels().then(r => r.models.map(m => m.model.name)).catch(() => [] as string[]),
      api.blends().then(r => r.blends).catch(() => []),
      api.dashboards().then(r => r.dashboards).catch(() => []),
    ]).then(([lineages, own, blends, dashboards]) => {
      if (stale) return
      setUses(usesOf(target, models, Object.fromEntries(names.map((n, i) => [n, lineages[i]])), { models: own, blends, dashboards }))
    })
    return () => { stale = true }
  }, [kind, name, models])

  const tick = (key: string, on: boolean) => setTicked(s => {
    const next = new Set(s)
    if (on) next.add(key)
    else next.delete(key)
    return next
  })
  const go = async () => {
    setBusy(true); setError(null)
    for (const s of deleteSteps({ kind, name }, ticked)) {
      try { await run(s) } catch (e) {
        setError(`Couldn't delete the ${s.kind} "${s.name}": ${(e as Error).message}. "${label}" and what comes after it weren't deleted.`)
        setBusy(false)
        return
      }
    }
    onDone()
  }

  const items = (kind: string, title: string, list: Item[]) => list.length > 0 && (
    <div className="delete-uses-group">
      <div className="field-group">{title}</div>
      {list.map(i => i.deletable
        ? <label key={i.name} className="check"><input type="checkbox" checked={ticked.has(`${kind}:${i.name}`)}
            onChange={e => tick(`${kind}:${i.name}`, e.target.checked)} /> {i.label}</label>
        : <div key={i.name} className="muted small">{i.label}: from the platform, it stays</div>)}
    </div>
  )
  const what = kind
  const used = uses && uses.models.length + uses.blends.length + uses.dashboards.length > 0
  return (
    <div className="modal-bg" onMouseDown={e => { if (e.target === e.currentTarget && !busy) onCancel() }}>
      <div className="modal delete-uses" role="dialog" aria-modal="true">
        <h2>Delete the {what} "{label}"?</h2>
        {note && <p className="muted small">{note}</p>}
        {!uses && <p className="muted">Looking for what uses it…</p>}
        {uses && !used && uses.unknown.length === 0 && <p className="muted">Nothing uses it.</p>}
        {used && <p>These use it. Tick what to delete with it. What you leave stops working, and works again
          as it was once a {what} of the same name is back.</p>}
        {uses && items('model', 'Models', uses.models)}
        {uses && items('blend', 'Blends', uses.blends)}
        {uses && uses.dashboards.length > 0 && (
          <div className="delete-uses-group">
            <div className="field-group">Dashboards</div>
            {uses.dashboards.map(d => (
              <label key={d.id} className="check"><input type="checkbox" checked={ticked.has(`dashboard:${d.id}`)}
                onChange={e => tick(`dashboard:${d.id}`, e.target.checked)} />
                {d.title} <span className="muted small">
                  {d.total !== null && d.used < d.total ? `${d.used} of its ${d.total} tiles; ticked, the whole dashboard goes` : d.used === 1 ? '1 tile' : `${d.used} tiles`}
                </span></label>
            ))}
          </div>
        )}
        {uses?.reports && <p className="muted small">Scheduled reports run on these dashboards.</p>}
        {uses && uses.unknown.length > 0 && <p className="warn">Couldn't check what uses {uses.unknown.join(', ')}.</p>}
        {error && <div className="error">{error}</div>}
        <div className="row dialog-buttons">
          <button disabled={busy} onClick={onCancel}>Cancel</button>
          <button className="danger primary" disabled={!uses || busy} onClick={go}>
            {ticked.size ? `Delete it and ${ticked.size} more` : 'Delete'}
          </button>
        </div>
      </div>
    </div>
  )
}
