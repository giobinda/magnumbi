import { useEffect, useRef, useState } from 'react'
import { AGG_LABELS, api } from '../api'
import { isTemporal } from '../api'
import type { DimType, Format, Formula, FormulaCheck, ModelInfo } from '../api'
import { FUNCTIONS, OPERATORS, RESULT_FUNCTIONS, ROW_FUNCTIONS, insertAt, nameFor, usesResults } from '../formula'

type Props = {
  model: ModelInfo
  /** the formula edited (none: a new one) */
  initial?: Formula | null
  /** the chart's other formulas, which this one may use */
  others?: Formula[]
  /** a new formula starts per row (a field) rather than on totals */
  startPerRow?: boolean
  /** in a chart: calculations on its results are possible too */
  inChart?: boolean
  /** names already used by the model's fields and the query's formulas */
  taken: string[]
  /** Org admins may share it with everyone, as one of the model's measures */
  canShare: boolean
  /** an edited shared formula stays shared */
  shared?: boolean
  /** `type`: what a per-row formula gives */
  onSave: (f: Formula, shared: boolean, type?: DimType) => Promise<void> | void
  onCancel: () => void
}

const numberColumns = (m: ModelInfo) => m.dimensions.filter(d => d.type === 'number' || d.type === 'string' || d.aggregation)
const aggOf = (d: ModelInfo['dimensions'][number]) => d.aggregation ?? (d.type === 'number' ? 'sum' : 'count_distinct')

/** Write a calculation from the model's measures and columns: click to insert, checked as you type. */
export default function FormulaEditor({ model, initial, others = [], startPerRow, inChart, taken, canShare, shared, onSave, onCancel }: Props) {
  const [perRow, setPerRow] = useState(initial ? Boolean(initial.per_row) : Boolean(startPerRow))
  // the others as text: a new array on each render isn't a change
  const othersJson = JSON.stringify(others)
  const [label, setLabel] = useState(initial?.label ?? '')
  const [expr, setExpr] = useState(initial?.expr ?? '')
  const [format, setFormat] = useState<Format>(initial?.format ?? 'number')
  const [share, setShare] = useState(Boolean(shared))
  const [checked, setCheck] = useState<FormulaCheck | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [saving, setSaving] = useState(false)
  const box = useRef<HTMLTextAreaElement>(null)

  // checked by the server (the same rules as the query), a moment after typing stops
  useEffect(() => {
    if (!expr.trim()) return
    const ctl = new AbortController()
    const t = setTimeout(() => {
      api.checkFormula(model.name, expr, perRow, JSON.parse(othersJson) as Formula[], ctl.signal).then(setCheck, e => {
        if ((e as Error).name !== 'AbortError') setCheck({ ok: false, error: (e as Error).message, at: 0 })
      })
    }, 300)
    return () => { clearTimeout(t); ctl.abort() }
  }, [expr, model.name, perRow, othersJson])

  const insert = (snippet: string) => {
    const el = box.current
    const [start, end] = el ? [el.selectionStart, el.selectionEnd] : [expr.length, expr.length]
    const next = insertAt(expr, start, end, snippet)
    setExpr(next.text)
    requestAnimationFrame(() => { el?.focus(); el?.setSelectionRange(next.cursor, next.cursor) })
  }

  const check = expr.trim() ? checked : null
  const onResults = !perRow && usesResults(expr)
  const name = initial?.name ?? nameFor(label, taken)
  const save = async () => {
    setSaving(true); setError(null)
    try {
      const f: Formula = perRow ? { name, label: label.trim(), expr: expr.trim(), per_row: true }
        : { name, label: label.trim(), expr: expr.trim(), format }
      await onSave(f, share && !onResults, check?.type)
    } catch (e) { setError((e as Error).message) } finally { setSaving(false) }
  }
  // on totals: the measures (the model's, and the chart's own); per row: the columns
  const measures = [
    ...model.measures.map(m => ({ name: m.name, label: m.label, formula: m.formula })),
    ...others.filter(f => !f.per_row).map(f => ({ name: f.name, label: f.label, formula: f.expr })),
  ].filter(m => m.name !== initial?.name)
  const columns = model.dimensions.filter(d => d.name !== initial?.name && !isTemporal(d.type))

  return (
    <div className="modal-bg" onMouseDown={e => { if (e.target === e.currentTarget) onCancel() }}>
      <div className="modal formula-editor">
        <h2>{initial ? 'Edit formula' : 'New formula'}</h2>
        <div className="formula-mode" role="radiogroup" aria-label="Calculated">
          <label className="check"><input type="radio" checked={!perRow} disabled={Boolean(initial)} onChange={() => setPerRow(false)} />
            <span><b>On totals</b>: a measure, e.g. <span className="mono">revenue / transactions</span></span></label>
          <label className="check"><input type="radio" checked={perRow} disabled={Boolean(initial)} onChange={() => setPerRow(true)} />
            <span><b>Per row</b>: a field to group, filter or add up, e.g. <span className="mono">amount * qty</span></span></label>
        </div>
        <div className="row wrap">
          <label className="grow">Name <input className="formula-name" autoFocus={!initial} value={label} placeholder="e.g. Revenue per sale"
            onChange={e => setLabel(e.target.value)} /></label>
          {!perRow && <label>Shown as
            <select value={format} onChange={e => setFormat(e.target.value as Format)}>
              <option value="number">number</option><option value="currency">currency</option><option value="percent">percent</option>
            </select>
          </label>}
        </div>
        <label>Formula
          <textarea ref={box} className="mono" rows={3} value={expr} spellCheck={false} autoFocus={Boolean(initial)}
            placeholder={perRow ? "e.g. amount * qty, or if(region = 'EMEA', 'home', 'away')" : 'e.g. revenue / transactions'} onChange={e => setExpr(e.target.value)} />
        </label>
        <div className={`formula-check ${check ? (check.ok ? 'fine' : 'bad') : ''}`} aria-live="polite">
          {!check ? <span className="muted">Click the fields and functions below, or type.</span>
            : check.ok ? `✓ Looks right${check.type ? ` (gives ${check.type === 'string' ? 'text' : check.type === 'boolean' ? 'yes or no' : 'a number'})` : ''}`
            : <>
                <span>{check.error}</span>
                {/* where the problem is */}
                <code className="where">{expr.slice(0, check.at ?? 0)}<mark>{expr.slice(check.at ?? 0, (check.at ?? 0) + 1) || ' '}</mark>{expr.slice((check.at ?? 0) + 1)}</code>
              </>}
        </div>

        <div className="palette">
          {perRow ? <>
            <div className="field-group">Columns</div>
            <div className="row wrap">
              {columns.map(d => (
                <button key={d.name} className="chip dimension" title={d.formula ? `${d.name} = ${d.formula}` : d.name}
                  onClick={() => insert(d.name)}>{d.formula ? 'ƒ ' : ''}{d.label ?? d.name}</button>
              ))}
            </div>
          </> : <>
            <div className="field-group">Measures</div>
            <div className="row wrap">
              {measures.map(m => (
                <button key={m.name} className="chip measure" title={m.formula ? `${m.name} = ${m.formula}` : m.name}
                  onClick={() => insert(m.name)}>{m.formula ? 'ƒ ' : ''}{m.label ?? m.name}</button>
              ))}
            </div>
            <div className="field-group">Columns</div>
            <div className="row wrap">
              {numberColumns(model).map(d => (
                <button key={d.name} className="chip dimension" title={`${aggOf(d)}(${d.name}): ${AGG_LABELS[aggOf(d)]}; sum, avg, min, max, count and count_distinct work too`}
                  onClick={() => insert(`${aggOf(d)}(${d.name})`)}>Σ {d.label ?? d.name}</button>
              ))}
            </div>
          </>}
          <div className="field-group">Functions</div>
          <div className="row wrap">
            {(perRow ? ROW_FUNCTIONS : FUNCTIONS).map(f => <button key={f.name} className="chip fn mono" title={f.help} onClick={() => insert(f.template)}>{f.name}</button>)}
            {OPERATORS.map(o => <button key={o} className="chip fn mono" onClick={() => insert(/\w/.test(o) ? ` ${o} ` : o)}>{o}</button>)}
            {perRow && <button className="chip fn mono" title="Text, in single quotes" onClick={() => insert("'|'")}>'text'</button>}
          </div>
          {inChart && !perRow && <>
            <div className="field-group">On the chart's results</div>
            <div className="row wrap">
              {RESULT_FUNCTIONS.map(f => <button key={f.name} className="chip fn mono" title={f.help} onClick={() => insert(f.template)}>{f.name}</button>)}
            </div>
          </>}
        </div>

        {canShare && !shared && !onResults && (
          <label className="check"><input type="checkbox" checked={share} onChange={e => setShare(e.target.checked)} />
            Share with everyone in the organisation, as one of this model's {perRow ? 'fields' : 'measures'}</label>
        )}
        {onResults && <p className="muted small">A calculation on this chart's results: it runs on the rows the chart shows,
          after its query, so it stays with the chart.</p>}
        {shared && <p className="muted small">Shared with everyone in the organisation: saving changes it for them too.</p>}
        {error && <div className="error">{error}</div>}
        <div className="row">
          <span className="muted small mono grow">{name}</span>
          <button onClick={onCancel}>Cancel</button>
          <button className="primary" disabled={saving || !label.trim() || !check?.ok} onClick={save}>{initial ? 'Save' : 'Add'}</button>
        </div>
      </div>
    </div>
  )
}
