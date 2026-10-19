import { useMemo, useState } from 'react'
import type { ReactNode } from 'react'
import { parse } from '../markdown'
import type { Inline } from '../markdown'

function inline(c: Inline[]): ReactNode[] {
  return c.map((x, i) => {
    switch (x.t) {
      case 'text': return x.v
      case 'code': return <code key={i}>{x.v}</code>
      case 'strong': return <strong key={i}>{inline(x.c)}</strong>
      case 'em': return <em key={i}>{inline(x.c)}</em>
      case 'link': return x.href.startsWith('/') || x.href.startsWith('#')
        ? <a key={i} href={x.href}>{inline(x.c)}</a>
        : <a key={i} href={x.href} target="_blank" rel="noreferrer">{inline(x.c)}</a>
    }
  })
}

/** Text tiles' markdown, as React elements (never as HTML). */
export function Markdown({ text }: { text: string }) {
  const blocks = useMemo(() => parse(text), [text])
  return (
    <div className="markdown">
      {blocks.map((b, i) => {
        switch (b.t) {
          case 'h': return b.level === 1 ? <h2 key={i}>{inline(b.c)}</h2> : b.level === 2 ? <h3 key={i}>{inline(b.c)}</h3> : <h4 key={i}>{inline(b.c)}</h4>
          case 'p': return <p key={i}>{inline(b.c)}</p>
          case 'hr': return <hr key={i} />
          case 'ul': return <ul key={i}>{b.items.map((it, j) => <li key={j}>{inline(it)}</li>)}</ul>
          case 'ol': return <ol key={i}>{b.items.map((it, j) => <li key={j}>{inline(it)}</li>)}</ol>
        }
      })}
    </div>
  )
}

type Props = {
  title?: string | null
  text: string
  editing?: boolean
  onEdit?: () => void
  onDelete?: () => void
  actions?: ReactNode
}

/** A tile of words: headings, notes, how to read the dashboard. */
export default function TextTile({ title, text, editing, onEdit, onDelete, actions }: Props) {
  return (
    <div className="tile text-tile">
      {(title || editing) && (
        <div className="tile-head">
          <div className="tile-title">{title}</div>
          {editing && <div className="tile-actions">
            {actions}
            {onEdit && <button className="icon" title="Edit text" onClick={onEdit}>✎</button>}
            {onDelete && <button className="icon" title="Remove tile" onClick={onDelete}>✕</button>}
          </div>}
        </div>
      )}
      <div className="tile-body"><Markdown text={text} /></div>
    </div>
  )
}

/** Writing a text tile, with a preview. */
export function TextEditor({ title: t0, text: x0, onSubmit, onCancel }: {
  title?: string | null; text?: string | null
  onSubmit: (title: string | null, text: string) => void; onCancel: () => void
}) {
  const [title, setTitle] = useState(t0 ?? '')
  const [text, setText] = useState(x0 ?? '')
  return (
    <div className="text-editor">
      <input className="title-input" placeholder="Title (optional)" value={title} onChange={e => setTitle(e.target.value)} />
      <div className="text-editor-panes">
        <textarea value={text} autoFocus rows={14} placeholder={'## A heading\n\nWhat this dashboard shows, **in bold** or *italics*.\n\n- a list\n- [a link](https://example.com)'}
          onChange={e => setText(e.target.value)} />
        <div className="text-preview"><Markdown text={text} /></div>
      </div>
      <p className="muted small">Markdown: <code>#</code> headings, <code>**bold**</code>, <code>*italics*</code>, <code>- lists</code>, <code>[links](https://…)</code>, <code>---</code> a line.</p>
      <div className="row">
        <button onClick={onCancel}>Cancel</button>
        <button className="primary" disabled={!text.trim()} onClick={() => onSubmit(title.trim() || null, text)}>Done</button>
      </div>
    </div>
  )
}
