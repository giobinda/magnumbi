/** A dashboard's tabs. While editing: add one, or remove one (its tiles move to the first tab). */
export default function Tabs({ tabs, current, onPick, onAdd, onRemove }: {
  tabs: string[]; current: string | null; onPick: (tab: string) => void
  onAdd?: (tab: string) => void; onRemove?: (tab: string) => void
}) {
  if (!tabs.length && !onAdd) return null
  const add = () => {
    const name = prompt(tabs.length ? 'Name of the new tab' : 'Tabs split a dashboard into pages. The tiles you have go on the first tab.\n\nName of the first tab', tabs.length ? '' : 'Overview')?.trim()
    if (!name) return
    if (tabs.includes(name)) { alert(`There is already a tab called "${name}".`); return }
    onAdd!(name)
  }
  return (
    <div className="tabs" role="tablist">
      {tabs.map(t => (
        <span key={t} className={t === current ? 'tab active' : 'tab'}>
          <button role="tab" aria-selected={t === current} onClick={() => onPick(t)}>{t}</button>
          {onRemove && <button className="icon" title={`Remove the tab "${t}" (its tiles move to the first tab)`} onClick={() => onRemove(t)}>✕</button>}
        </span>
      ))}
      {onAdd && <button className="link" onClick={add}>+ Tab</button>}
    </div>
  )
}

/** While editing: send a tile to another tab. */
export function MoveTab({ tabs, current, onMove }: { tabs: string[]; current: string | null; onMove: (tab: string) => void }) {
  return (
    <select className="move-tab" title="Move to another tab" value="" onChange={e => e.target.value && onMove(e.target.value)}>
      <option value="">⇄</option>
      {tabs.filter(t => t !== current).map(t => <option key={t} value={t}>Move to “{t}”</option>)}
    </select>
  )
}
