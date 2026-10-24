import { createContext, useCallback, useContext, useEffect, useLayoutEffect, useRef, useState } from 'react'
import { useLocation, useNavigate } from 'react-router-dom'
import { api, getViewAs } from '../api'
import { useAuth } from '../auth'
import { placeBalloon, welcomeTour } from '../tour'
import type { TourStep } from '../tour'

interface Running { steps: TourStep[]; at: number; onEnd?: () => void }
const TourContext = createContext<{ start: (steps: TourStep[], onEnd?: () => void) => void }>({ start: () => {} })

/** Starts a tour: balloons next to the controls its steps name. */
export const useTour = () => useContext(TourContext)

const LATER_KEY = 'magnum.tour.later'

export function TourProvider({ children }: { children: React.ReactNode }) {
  const [run, setRun] = useState<Running | null>(null)
  const start = useCallback((steps: TourStep[], onEnd?: () => void) => setRun({ steps, at: 0, onEnd }), [])
  const end = () => { run?.onEnd?.(); setRun(null) }
  const go = (at: number) => (at >= run!.steps.length ? end() : setRun({ ...run!, at: Math.max(0, at) }))
  return (
    <TourContext.Provider value={{ start }}>
      {children}
      {run && <Balloon key={run.at} step={run.steps[run.at]} at={run.at} count={run.steps.length}
        onNext={() => go(run.at + 1)} onBack={() => go(run.at - 1)} onEnd={end} />}
    </TourContext.Provider>
  )
}

/** The welcome tour, on a dashboard: the one open, else the first. False when there's none yet. */
export function useWelcomeTour() {
  const { auth } = useAuth()
  const { start } = useTour()
  const { pathname } = useLocation()
  return useCallback(async () => {
    const [list, chat] = await Promise.all([api.dashboards(), api.chat().catch(() => ({ enabled: false }))])
    const open = pathname.match(/^\/dashboards\/([^/]+)$/)?.[1]
    const id = open ?? list.dashboards[0]?.id
    if (!id) return false
    const [first, ...rest] = welcomeTour(auth?.permissions ?? [], chat.enabled)
    start([{ ...first, path: `/dashboards/${encodeURIComponent(id)}` }, ...rest])
    api.tourSeen(true).catch(() => {})  // taken: not offered again, even if left halfway
    return true
  }, [auth, start, pathname])
}

/** First sign-in (in each organisation): would they like the tour? Asked until they take it,
 *  skip it or say never; "Not now" waits for the next sign-in. Not while previewing a role. */
export function WelcomeOffer() {
  const { auth } = useAuth()
  const welcome = useWelcomeTour()
  const [ask, setAsk] = useState(false)
  useEffect(() => {
    let later = false
    try { later = sessionStorage.getItem(LATER_KEY) === '1' } catch { /* private mode */ }
    if (!auth?.role || getViewAs() || later) return
    Promise.all([api.tour(), api.dashboards()])
      .then(([t, d]) => setAsk(t.offer && d.dashboards.length > 0), () => setAsk(false))
  }, [auth?.role, auth?.org?.id])
  if (!ask) return null
  const notNow = () => { setAsk(false); try { sessionStorage.setItem(LATER_KEY, '1') } catch { /* private mode */ } }
  const never = () => { setAsk(false); api.tourSeen(true).catch(() => {}) }
  return (
    <div className="modal-bg" onMouseDown={e => { if (e.target === e.currentTarget) notNow() }}>
      <div className="modal welcome" role="dialog" aria-labelledby="welcome-title">
        <h2 id="welcome-title">Welcome to Magnum BI{auth?.org?.name ? `, ${auth.org.name}` : ''}</h2>
        <p>Would you like a one-minute tour? It shows you around a dashboard: the period, filters, drilling down, and what
          else your role lets you do.</p>
        <div className="row">
          <button className="primary" autoFocus onClick={() => { setAsk(false); welcome() }}>Show me around</button>
          <button onClick={notNow}>Not now</button>
          <span className="grow" />
          <button className="link" onClick={never}>Don’t ask again</button>
        </div>
      </div>
    </div>
  )
}

const VISIBLE = (el: Element) => el.getClientRects().length > 0

/** One step: goes to its page, waits for its control (up to 3 s, else the next step), and puts
 *  a balloon next to it, with a ring around it. Escape ends the tour. */
function Balloon({ step, at, count, onNext, onBack, onEnd }:
  { step: TourStep; at: number; count: number; onNext: () => void; onBack: () => void; onEnd: () => void }) {
  const navigate = useNavigate()
  const { pathname } = useLocation()
  const [rect, setRect] = useState<DOMRect | null>(null)
  const [size, setSize] = useState({ width: 320, height: 150 })
  const ref = useRef<HTMLDivElement>(null)

  const went = useRef(false)
  useEffect(() => {
    if (!went.current && step.path && pathname !== step.path) { went.current = true; navigate(step.path) }
  }, [step.path, pathname, navigate])
  useEffect(() => {
    let found: Element | null = null, waited = 0
    const measure = () => { if (found) setRect(found.getBoundingClientRect()) }
    const look = setInterval(() => {
      if (!found) {
        found = [...document.querySelectorAll(`[data-tour="${step.target}"]`)].find(VISIBLE) ?? null
        if (found) { found.scrollIntoView({ block: 'center', inline: 'nearest' }); measure() }
        else if ((waited += 100) > (step.path ? 3000 : 1500)) { clearInterval(look); onNext() }  // not for this role
      } else measure()  // it can move (charts load, menus open)
    }, 100)
    addEventListener('resize', measure); addEventListener('scroll', measure, true)
    return () => { clearInterval(look); removeEventListener('resize', measure); removeEventListener('scroll', measure, true) }
  }, [step.target, step.path, onNext])
  useLayoutEffect(() => {
    const b = ref.current?.getBoundingClientRect()
    if (b && (b.height !== size.height)) setSize({ width: 320, height: b.height })
  }, [rect, size.height])
  useEffect(() => {
    const esc = (e: KeyboardEvent) => { if (e.key === 'Escape') onEnd() }
    addEventListener('keydown', esc)
    return () => removeEventListener('keydown', esc)
  }, [onEnd])
  if (!rect) return null
  const p = placeBalloon(rect, size, { width: innerWidth, height: innerHeight })
  return (
    <>
      <div className="tour-ring" style={{ top: rect.top - 4, left: rect.left - 4, width: rect.width + 8, height: rect.height + 8 }} />
      <div ref={ref} className={`tour-balloon ${p.side}`} role="dialog" aria-live="polite" aria-labelledby="tour-title"
        style={{ top: p.top, left: p.left, width: p.width, ['--arrow' as string]: `${p.arrow}px` }}>
        <div className="row"><b id="tour-title">{step.title}</b><span className="grow" /><span className="muted small">{at + 1} of {count}</span></div>
        <p>{step.text}</p>
        <div className="row">
          <button className="link small" onClick={onEnd}>{at + 1 < count ? 'Skip tour' : 'Close'}</button>
          <span className="grow" />
          {at > 0 && <button className="small" onClick={onBack}>Back</button>}
          <button className="primary small" autoFocus onClick={onNext}>{at + 1 < count ? 'Next' : 'Done'}</button>
        </div>
      </div>
    </>
  )
}
