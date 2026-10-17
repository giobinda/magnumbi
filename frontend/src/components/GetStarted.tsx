import { useEffect, useState } from 'react'
import { Link } from 'react-router-dom'
import { api } from '../api'
import type { SetupStep } from '../api'

/** The website, where the how-to guides are. */
export const SITE = 'https://giobinda.github.io/magnumbi/'
const HIDE_KEY = 'magnum.get-started.hidden'

/** "Get started": what's left to set up, for the platform's owners or the organisation's
 *  admins, each step linked to where it's done and to its guide. Shown until the required steps
 *  are done (or hidden). */
export default function GetStarted() {
  const [data, setData] = useState<{ for: string | null; steps: SetupStep[] } | null>(null)
  const [hidden, setHidden] = useState(() => { try { return localStorage.getItem(HIDE_KEY) === '1' } catch { return false } })
  useEffect(() => { api.setup().then(setData, () => setData(null)) }, [])
  if (!data?.for || !data.steps.length) return null
  const required = data.steps.filter(s => !s.optional)
  const done = required.filter(s => s.done).length
  if (done === required.length || hidden) return null
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
        {data.steps.map(s => (
          <li key={s.id} className={s.done ? 'done' : ''}>
            <span className="tick">{s.done ? '✓' : ''}</span>
            <div className="grow">
              <b>{s.title}</b>{s.optional && <span className="muted small"> · optional</span>}
              <div className="muted small">{s.detail}</div>
            </div>
            {!s.done && <Link className="btn small" to={s.link}>Go</Link>}
            <a className="small" href={SITE + s.guide} target="_blank" rel="noreferrer">How ↗</a>
          </li>
        ))}
      </ol>
    </div>
  )
}
