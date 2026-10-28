import { useCallback, useEffect, useState } from 'react'
import { Link } from 'react-router-dom'
import { api } from '../api'
import { SKIPPABLE, cardShown, shownSteps } from '../getStarted'
import { SHOW_ME } from '../tour'
import DemoCard from './DemoCard'
import { useTour } from './Tour'
import type { SetupStep } from '../api'

/** The website, where the how-to guides are. */
export const SITE = 'https://giobinda.github.io/magnumbi/'
const HIDE_KEY = 'magnum.get-started.hidden'
const SKIP_KEY = 'magnum.get-started.skipped'

const readSkipped = (): string[] => {
  try { return JSON.parse(localStorage.getItem(SKIP_KEY) ?? '[]') as string[] } catch { return [] }
}

/** "Get started": what's left to set up, for the platform's owners or the organisation's
 *  admins, each step linked to where it's done and to its guide. Shown until the required steps
 *  are done (or hidden). */
export default function GetStarted() {
  const [data, setData] = useState<{ for: string | null; steps: SetupStep[] } | null>(null)
  const [hidden, setHidden] = useState(() => { try { return localStorage.getItem(HIDE_KEY) === '1' } catch { return false } })
  const { start } = useTour()
  const [demo, setDemo] = useState(false)
  const [skipped, setSkipped] = useState(readSkipped)
  const reload = useCallback(() => { api.setup().then(setData, () => setData(null)) }, [])
  useEffect(() => {
    reload()
    // a step done in another tab or window ticks when this one is back
    window.addEventListener('focus', reload)
    return () => window.removeEventListener('focus', reload)
  }, [reload])
  if (!data?.for || !data.steps.length) return null
  const required = data.steps.filter(s => !s.optional)
  const done = required.filter(s => s.done).length
  if (!cardShown(data.steps, hidden, demo)) return null
  const skip = (id: string) => {
    const next = [...skipped, id]
    setSkipped(next)
    try { localStorage.setItem(SKIP_KEY, JSON.stringify(next)) } catch { /* private mode */ }
  }
  const hide = () => { setHidden(true); try { localStorage.setItem(HIDE_KEY, '1') } catch { /* private mode */ } }
  const guide = data.for === 'platform' ? 'guides/platform-owner.html' : 'guides/org-admin.html'
  return (
    <div className="card get-started">
      <div className="row">
        <h2>Get started</h2>
        <span className="muted small">{done} of {required.length} done</span>
        <span className="grow" />
        <a className="small" href={SITE + guide} target="_blank" rel="noreferrer">The full guide ↗</a>
        <button className="link small" onClick={hide}>Hide</button>
      </div>
      <div className="progress"><i style={{ width: `${(done / required.length) * 100}%` }} /></div>
      <ol className="steps-list">
        {shownSteps(data.steps, skipped).map(s => (
          <li key={s.id} className={s.done ? 'done' : ''}>
            <span className="tick">{s.done ? '✓' : ''}</span>
            <div className="grow">
              <b>{s.title}</b>{s.optional && <span className="muted small"> · optional</span>}
              <div className="muted small">{s.detail}</div>
            </div>
            {!s.done && SHOW_ME[s.id] && <button className="small" onClick={() => start(SHOW_ME[s.id])}>Show me</button>}
            {/* the sample data is set up right here */}
            {!s.done && s.id === 'demo' && <button className="small" onClick={() => setDemo(!demo)}>{demo ? 'Close' : 'Set up'}</button>}
            {!s.done && s.id !== 'demo' && <Link className="btn small" to={s.link}>Go</Link>}
            {!s.done && SKIPPABLE.includes(s.id) && <button className="link small" onClick={() => skip(s.id)}>Skip</button>}
            <a className="small" href={SITE + s.guide} target="_blank" rel="noreferrer">How ↗</a>
            {s.id === 'demo' && demo && <div className="step-demo"><DemoCard months={1} onDone={reload} /></div>}
          </li>
        ))}
      </ol>
    </div>
  )
}
