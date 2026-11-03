import { useEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { isLong, nextActive, pickable, placePopover } from '../search'
import type { Option } from '../search'

type Props = {
  value: string
  options: Option[]
  onChange: (value: string) => void
  /** the choice of nothing (value ''): "Choose a model…" */
  empty?: string
  /** its name, for those who can't see what it's next to */
  label?: string
  title?: string
  className?: string
  disabled?: boolean
}

const groupsOf = (options: Option[]) => [...new Set(options.map(o => o.group ?? ''))]

/** A choice among what's in the data (models, tables, fields…): a plain select while the list is
 *  short, with a search box once it's long. Options wrap: a long name is read whole. */
export default function Picker(props: Props) {
  if (isLong(props.options.length)) return <LongPicker {...props} />
  const { value, options, onChange, empty, label, title, className, disabled } = props
  const groups = groupsOf(options)
  const list = (os: Option[]) => os.map(o => <option key={o.value} value={o.value} disabled={o.disabled}>{o.label}</option>)
  return (
    <select className={className} value={value} aria-label={label} title={title} disabled={disabled} onChange={e => onChange(e.target.value)}>
      {empty !== undefined && <option value="">{empty}</option>}
      {groups.length > 1
        ? groups.map(g => <optgroup key={g} label={g}>{list(options.filter(o => (o.group ?? '') === g))}</optgroup>)
        : list(options)}
    </select>
  )
}

function LongPicker({ value, options, onChange, empty, label, title, className, disabled }: Props) {
  const [place, setPlace] = useState<ReturnType<typeof placePopover> | null>(null)
  const [query, setQuery] = useState('')
  const [active, setActive] = useState(0)
  const button = useRef<HTMLButtonElement>(null)
  const pop = useRef<HTMLDivElement>(null)
  const open = place !== null
  const close = () => { setPlace(null); button.current?.focus() }

  useEffect(() => {
    if (!open) return
    const inside = (e: Event) => Boolean(pop.current?.contains(e.target as Node) || button.current?.contains(e.target as Node))
    const away = (e: Event) => { if (!inside(e)) setPlace(null) }
    // it stays where it opened: moving the page under it closes it
    document.addEventListener('mousedown', away)
    window.addEventListener('scroll', away, true)
    window.addEventListener('resize', away)
    return () => {
      document.removeEventListener('mousedown', away)
      window.removeEventListener('scroll', away, true)
      window.removeEventListener('resize', away)
    }
  }, [open])

  const toggle = () => {
    if (open) { setPlace(null); return }
    setQuery(''); setActive(0)
    setPlace(placePopover(button.current!.getBoundingClientRect(), { width: window.innerWidth, height: window.innerHeight }))
  }
  const all: Option[] = empty !== undefined ? [{ value: '', label: empty }, ...options] : options
  const found = pickable(all, query)
  const pick = (o: Option) => { if (!o.disabled) { onChange(o.value); close() } }
  const keys = (e: React.KeyboardEvent) => {
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault()
      setActive(a => nextActive(found, a, e.key === 'ArrowDown' ? 1 : -1))
    } else if (e.key === 'Enter') {
      e.preventDefault()
      if (found[active]) pick(found[active])
    } else if (e.key === 'Escape') {
      e.preventDefault()
      e.stopPropagation()
      close()
    }
  }
  const current = all.find(o => o.value === value)
  return (
    <span className={`picker${className ? ` ${className}` : ''}`}>
      <button ref={button} type="button" className="picker-btn" aria-haspopup="listbox" aria-expanded={open} aria-label={label} title={title}
        disabled={disabled} onClick={toggle}>
        <span className={current?.value ? '' : 'muted'}>{current?.label ?? value}</span> <span aria-hidden>▾</span>
      </button>
      {place && createPortal(
        <div ref={pop} className="picker-pop" style={place}>
          <input autoFocus type="search" value={query} placeholder="Search…" aria-label={`Search ${label ?? 'the list'}`} onKeyDown={keys}
            onChange={e => { setQuery(e.target.value); setActive(0) }} />
          <div className="picker-list" role="listbox">
            {found.length === 0 && <div className="muted small picker-none">Nothing matches.</div>}
            {found.map((o, i) => (
              <div key={o.value}>
                {o.group && o.group !== found[i - 1]?.group && <div className="picker-group">{o.group}</div>}
                <button type="button" role="option" aria-selected={o.value === value} disabled={o.disabled}
                  className={`picker-option${i === active ? ' active' : ''}${o.value === value ? ' current' : ''}`}
                  onMouseEnter={() => setActive(i)} onClick={() => pick(o)}>
                  {o.label}{o.hint && <span className="muted small picker-hint">{o.hint}</span>}
                </button>
              </div>
            ))}
          </div>
        </div>,
        document.body,
      )}
    </span>
  )
}
