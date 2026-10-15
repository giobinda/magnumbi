import type { ModelInfo, RowRule } from '../api'
import { MultiSelect } from './FilterBar'

// a filter on a base model also covers the models extending it: offer the base models
const choices = (models: ModelInfo[]) => (models.some(m => !m.extends) ? models.filter(m => !m.extends) : models)

/** Row filters: each one is model → dimension → allowed values. */
export default function Restrictions({ rules, models, onChange, addLabel = '+ Add restriction' }: {
  rules: RowRule[]; models: ModelInfo[]; onChange: (r: RowRule[]) => void; addLabel?: string
}) {
  const update = (i: number, patch: Partial<RowRule>) => onChange(rules.map((r, j) => (j === i ? { ...r, ...patch } : r)))
  return (
    <div className="restrictions">
      {rules.map((r, i) => {
        const model = models.find(m => m.name === r.model)
        const extended = models.filter(m => m.extends === r.model).map(m => m.label)
        return (
          <div key={i} className="rule">
            <select value={r.model} onChange={e => update(i, { model: e.target.value, dimension: '', values: [] })}>
              <option value="">Model…</option>
              {choices(models).map(m => <option key={m.name} value={m.name}>{m.label}</option>)}
            </select>
            <span className="muted">›</span>
            <select value={r.dimension} disabled={!model} onChange={e => update(i, { dimension: e.target.value, values: [] })}>
              <option value="">Dimension…</option>
              {model?.dimensions.map(d => <option key={d.name} value={d.name}>{d.label || d.name}</option>)}
            </select>
            <span className="muted">is one of</span>
            {model && r.dimension
              ? <MultiSelect key={`${r.model}.${r.dimension}`} model={r.model} field={r.dimension} values={r.values} onChange={values => update(i, { values })} />
              : <span className="muted small">pick a dimension first</span>}
            {model && r.dimension && r.values.length === 0 && <span className="small bad">no access</span>}
            <button className="icon" title="Remove restriction" onClick={() => onChange(rules.filter((_, j) => j !== i))}>✕</button>
            {extended.length > 0 && <span className="muted small rule-note">also applies to {extended.join(', ')}</span>}
          </div>
        )
      })}
      <button className="link" onClick={() => onChange([...rules, { model: '', dimension: '', values: [] }])}>{addLabel}</button>
    </div>
  )
}
