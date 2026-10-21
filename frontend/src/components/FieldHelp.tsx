import { useEffect, useRef } from 'react'
import { Link } from 'react-router-dom'
import type { Dimension, GlossaryTerm, Measure } from '../api'
import { definitions } from '../governance'

type Props = {
  field: (Dimension | Measure) & { formula?: string | null }
  kind: 'dimension' | 'measure'
  /** null: still loading */
  glossary: GlossaryTerm[] | null
  onClose: () => void
}

const TYPES: Record<string, string> = {
  string: 'Text', number: 'Number', boolean: 'Yes / no', date: 'Date', time: 'Date and time', datetime: 'Date and time',
}

/** What the catalog says about a field: its meaning, other names, examples, terms and tags. */
export default function FieldHelp({ field, kind, glossary, onClose }: Props) {
  const panel = useRef<HTMLDivElement>(null)
  // closes on Escape, or a click anywhere else
  useEffect(() => {
    const key = (e: KeyboardEvent) => { if (e.key === 'Escape') onClose() }
    const click = (e: MouseEvent) => {
      const t = e.target as HTMLElement
      if (!panel.current?.contains(t) && !t.closest('.field-help-btn')) onClose()
    }
    document.addEventListener('keydown', key)
    document.addEventListener('mousedown', click)
    return () => { document.removeEventListener('keydown', key); document.removeEventListener('mousedown', click) }
  }, [onClose])
  const what = kind === 'measure' ? 'Measure' : `${TYPES[(field as Dimension).type] ?? 'Field'} field`
  const terms = definitions(field.terms, glossary ?? [])
  return (
    <div ref={panel} className="field-help" role="dialog" aria-label={`About ${field.label ?? field.name}`}>
      <div className="field-help-head">
        <b>{field.label ?? field.name}</b>
        <span className="muted small">{what} · <span className="mono">{field.name}</span></span>
      </div>
      {field.formula && <p className="mono small">= {field.formula}</p>}
      {field.description ? <p>{field.description}</p> : <p className="muted">No description yet.</p>}
      {!!field.synonyms?.length && <p className="small"><b>Also called:</b> {field.synonyms.join(', ')}</p>}
      {!!field.examples?.length && <p className="small"><b>For example:</b> {field.examples.join('; ')}</p>}
      {terms.length > 0 && (
        <div className="small">
          <b>Glossary</b>
          {terms.map(t => (
            <p key={t.name} className="term"><i>{t.name}</i>{glossary === null ? '' : t.definition ? `: ${t.definition}` : ' (no longer in the glossary)'}</p>
          ))}
        </div>
      )}
      {!!field.tags?.length && <p>{field.tags.map(t => <span key={t} className="tp-badge">{t}</span>)}</p>}
      <p className="small"><Link to="/catalog" target="_blank" rel="noopener">Open the catalog</Link></p>
    </div>
  )
}
