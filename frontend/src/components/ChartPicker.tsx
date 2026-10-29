import { useEffect, useMemo, useRef, useState } from 'react'
import type { ChartType } from '../api'
import { GALLERY } from '../chartGallery'
import Chart from './Chart'

/** The tile editor's chart: one small button, the chart chosen; it opens every chart, each shown
 *  on made-up data with what it's for. */
export default function ChartPicker({ type, onPick }: { type: ChartType; onPick: (t: ChartType) => void }) {
  const [open, setOpen] = useState(false)
  const box = useRef<HTMLDivElement>(null)
  const samples = useMemo(() => GALLERY.map(c => ({ ...c, result: c.sample() })), [])
  const current = GALLERY.find(c => c.type === type) ?? GALLERY[0]
  useEffect(() => {
    if (!open) return
    const away = (e: MouseEvent) => { if (!box.current?.contains(e.target as Node)) setOpen(false) }
    const esc = (e: KeyboardEvent) => { if (e.key === 'Escape') setOpen(false) }
    document.addEventListener('mousedown', away); document.addEventListener('keydown', esc)
    return () => { document.removeEventListener('mousedown', away); document.removeEventListener('keydown', esc) }
  }, [open])
  return (
    <div className="chart-picker" ref={box}>
      <button type="button" className="chart-picker-btn" aria-haspopup="dialog" aria-expanded={open} onClick={() => setOpen(!open)}>
        <span aria-hidden>{current.icon}</span> {current.label} <span aria-hidden>▾</span>
      </button>
      {open && (
        <div className="chart-gallery" role="dialog" aria-label="Chart types">
          {samples.map(c => (
            <button key={c.type} type="button" className={`chart-card${c.type === type ? ' active' : ''}`}
              onClick={() => { onPick(c.type); setOpen(false) }}>
              <div className="chart-card-preview" aria-hidden>
                <Chart result={c.result} type={c.type} options={c.type === 'gauge' ? { target: 100000 } : {}} />
              </div>
              <b>{c.label}</b>
              <span className="muted small">{c.use}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  )
}
