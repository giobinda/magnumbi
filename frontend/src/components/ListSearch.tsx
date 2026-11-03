import { isLong } from '../search'

/** The search box of a list on a page, once it's long; says how many it found. */
export default function ListSearch({ total, shown, value, onChange, what }: {
  total: number; shown: number; value: string; onChange: (q: string) => void
  /** what's listed: "dashboards" */
  what: string
}) {
  if (!isLong(total)) return null
  return (
    <div className="list-search">
      <input type="search" value={value} placeholder={`Search ${what}…`} aria-label={`Search ${what}`} onChange={e => onChange(e.target.value)} />
      {value.trim() && <span className="muted small">{shown} of {total}</span>}
    </div>
  )
}
