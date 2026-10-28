import { useEffect, useRef, useState } from 'react'
import { BrowserRouter, NavLink, Route, Routes, useLocation } from 'react-router-dom'
import { AuthGate, useAuth, useCan } from './auth'
import { SecurityCard } from './components/Security'
import GetStarted from './components/GetStarted'
import { TourProvider, WelcomeOffer, useWelcomeTour } from './components/Tour'
import { api, getOrg, getViewAs, setOrg, setViewAs } from './api'
import type { Role } from './api'
import MembersPage from './pages/MembersPage'
import OrganisationsPage from './pages/OrganisationsPage'
import { ThemeSwatch, useTheme } from './theme'
import DashboardPage from './pages/DashboardPage'
import PublicPage from './pages/PublicPage'
import GuestPage from './pages/GuestPage'
import ExplorePage from './pages/ExplorePage'
import Home from './pages/Home'
import UsagePage from './pages/UsagePage'
import BlendsPage from './pages/BlendsPage'
import ConnectionsPage from './pages/ConnectionsPage'
import DeliveryPage from './pages/DeliveryPage'
import UnsubscribePage from './pages/UnsubscribePage'
import ThemesPage from './pages/ThemesPage'
import ModelsPage from './pages/ModelsPage'
import CatalogPage from './pages/CatalogPage'
import ChatPage from './pages/ChatPage'
import ChatSettingsPage from './pages/ChatSettingsPage'
import CodePage from './pages/CodePage'

const ROLE_LABELS: Record<string, string> = { viewer: 'Viewer', user: 'User', power_user: 'Power user', org_admin: 'Org admin', system_owner: 'System owner' }

export default function App() {
  return (
    <BrowserRouter>
      <Routes>
        {/* public links: no sign-in */}
        <Route path="/s/:org/:token" element={<PublicPage />} />
        <Route path="/g/:org/:token" element={<GuestPage />} />
        <Route path="/unsubscribe" element={<UnsubscribePage />} />
        <Route path="*" element={<SignedIn />} />
      </Routes>
    </BrowserRouter>
  )
}

function SignedIn() {
  return (
      <AuthGate>
        <Routes>
          <Route path="/embed/:id" element={<DashboardPage embed />} />
          <Route path="*" element={
            <TourProvider>
            <div className="app">
              <nav className="topnav">
                <NavLink to="/" className="brand">Magnum<span>BI</span></NavLink>
                <ChatLink />
                <NavMenu label="Discover" tour="nav-discover" paths={['/dashboards', '/explore', '/catalog']}>
                  <NavLink to="/dashboards">Dashboards</NavLink>
                  <Guarded perm="explore"><NavLink to="/explore">Explore</NavLink></Guarded>
                  <NavLink to="/catalog">Catalog</NavLink>
                </NavMenu>
                <DataMenu />
                <SettingsMenu />
                <div className="nav-right"><OrgSwitcher /><ThemePicker /><UserMenu /></div>
              </nav>
              <Routes>
                <Route path="/" element={<Start />} />
                <Route path="/get-started" element={<div className="page narrow"><GetStarted always /></div>} />
                <Route path="/dashboards" element={<Home />} />
                <Route path="/chat" element={<ChatPage />} />
                <Route path="/chat-settings" element={<Guarded perm="manage_members" page><ChatSettingsPage /></Guarded>} />
                <Route path="/explore" element={<Guarded perm="explore" page><ExplorePage /></Guarded>} />
                <Route path="/blends" element={<Guarded perm="explore" page><BlendsPage /></Guarded>} />
                <Route path="/themes" element={<Guarded perm="manage_members" page><ThemesPage /></Guarded>} />
                <Route path="/delivery" element={<Guarded perm="manage_members" page><DeliveryPage /></Guarded>} />
                <Route path="/connections" element={<Guarded perm="manage_members" page><ConnectionsPage /></Guarded>} />
                <Route path="/catalog" element={<CatalogPage />} />
                <Route path="/models" element={<Guarded perm="manage_members" page><ModelsPage /></Guarded>} />
                <Route path="/usage" element={<Guarded perm="manage_members" page><UsagePage /></Guarded>} />
                <Route path="/code" element={<Guarded perm="manage_members" page><CodePage /></Guarded>} />
                <Route path="/members" element={<Guarded perm="manage_members" page><MembersPage /></Guarded>} />
                <Route path="/organisations" element={<Guarded perm="manage_orgs" page><OrganisationsPage /></Guarded>} />
                <Route path="/dashboards/:id" element={<DashboardPage />} />
              </Routes>
              <Credit />
              <WelcomeOffer />
            </div>
            </TourProvider>
          } />
        </Routes>
      </AuthGate>
  )
}

/** Attribution required by the license's additional terms (NOTICE): keep it visible in forks. */
function Credit() {
  return <footer className="app-credit">Powered by Magnum BI · © Giovanni Andrea Binda · AGPL-3.0</footer>
}

/** A group of the top bar's links: a button that opens them (closes on a pick, a click elsewhere
 *  or Escape). Hidden when the role can see none of them. */
function NavMenu({ label, paths, tour, children }: { label: string; paths: string[]; tour?: string; children: React.ReactNode }) {
  const [open, setOpen] = useState(false)
  const ref = useRef<HTMLDivElement>(null)
  const { pathname } = useLocation()
  useEffect(() => {
    if (!open) return
    const away = (e: MouseEvent) => { if (!ref.current?.contains(e.target as Node)) setOpen(false) }
    const esc = (e: KeyboardEvent) => { if (e.key === 'Escape') setOpen(false) }
    document.addEventListener('mousedown', away); document.addEventListener('keydown', esc)
    return () => { document.removeEventListener('mousedown', away); document.removeEventListener('keydown', esc) }
  }, [open])
  const active = paths.some(p => pathname === p || pathname.startsWith(p + '/'))
  return (
    <div className="nav-menu" ref={ref} data-tour={tour}>
      <button type="button" className={active ? 'active' : ''} aria-haspopup="true" aria-expanded={open} onClick={() => setOpen(!open)}>
        {label} <span aria-hidden>▾</span>
      </button>
      <div className="nav-menu-list" hidden={!open} onClick={() => setOpen(false)}>{children}</div>
    </div>
  )
}

/** Data: where it comes from and what it means (Org admins; system owners, for the platform). */
function DataMenu() {
  const can = useCan()
  if (!can('explore') && !can('manage_members')) return null
  return (
    <NavMenu label="Data" tour="nav-data" paths={['/connections', '/models', '/blends']}>
      <Guarded perm="manage_members"><NavLink to="/connections">Connections</NavLink></Guarded>
      <Guarded perm="manage_members"><NavLink to="/models">Models</NavLink></Guarded>
      <Guarded perm="explore"><NavLink to="/blends">Blends</NavLink></Guarded>
    </NavMenu>
  )
}

/** Settings: people, look, delivery, usage, code and (system owners) organisations. */
function SettingsMenu() {
  const can = useCan()
  const { auth } = useAuth()
  if (!can('manage_members')) return null
  return (
    <NavMenu label="Settings" tour="nav-settings" paths={['/members', '/themes', '/delivery', '/usage', '/code', '/organisations', '/chat-settings']}>
      {auth?.org && <NavLink to="/members">Members</NavLink>}
      {!auth?.lite && <Guarded perm="manage_orgs"><NavLink to="/organisations">Organisations</NavLink></Guarded>}
      <NavLink to="/themes">Themes</NavLink>
      <NavLink to="/delivery">Email &amp; webhooks</NavLink>
      <NavLink to="/chat-settings">Chat settings</NavLink>
      <NavLink to="/usage">Usage</NavLink>
      <NavLink to="/code">Code</NavLink>
    </NavMenu>
  )
}

/** System owners: work in the platform workspace (templates, every model) or inside an organisation. */
function OrgSwitcher() {
  const { auth } = useAuth()
  if (auth?.lite) return null // no organisations
  if (!auth?.orgs) return auth?.org ? <span className="org-badge" title="Your organisation">{auth.org.name}</span> : null
  const pick = (id: string) => { setOrg(id || null); location.assign('/') }  // dashboards differ per organisation
  return (
    <select className="org-select" title="Organisation you're working in" value={auth.org?.id ?? getOrg() ?? ''}
      onChange={e => pick(e.target.value)}>
      <option value="">Platform workspace</option>
      {auth.orgs.map(o => <option key={o.id} value={o.id}>{o.name}</option>)}
    </select>
  )
}

function ThemePicker() {
  const { themes, theme, preference, setPreference, override } = useTheme()
  const pinned = override && override === theme.id
  return (
    <label className="theme-picker" title={pinned ? `This dashboard uses the "${theme.label}" theme` : 'Theme'}>
      <ThemeSwatch theme={theme} />
      <select value={pinned ? `pinned:${theme.id}` : preference} disabled={!!pinned} onChange={e => setPreference(e.target.value)}>
        {pinned && <option value={`pinned:${theme.id}`}>{theme.label} (set by dashboard)</option>}
        <option value="auto">Auto (system)</option>
        {themes.map(t => <option key={t.id} value={t.id}>{t.label}</option>)}
      </select>
    </label>
  )
}

/** Hides UI the current role can't use (the server enforces the same rules regardless). */
function Guarded({ perm, page, children }: { perm: Parameters<ReturnType<typeof useCan>>[0]; page?: boolean; children: React.ReactNode }) {
  const can = useCan()
  if (can(perm)) return <>{children}</>
  return page ? <div className="page"><div className="warn">Your role can't open this page.</div></div> : null
}

function UserMenu() {
  const { auth, signOut, refresh } = useAuth()
  const [changing, setChanging] = useState(false)
  if (!auth?.role) return null
  const options = auth.view_as ?? []
  // a preview of a lower role ("view as"); the server never lets it go up
  const pick = (r: string) => { setViewAs(r !== auth.actual_role ? (r as Role) : null); refresh() }
  // lite: one person, no roles to preview
  const roleBadge = auth.lite ? null : options.length > 1 ? (
    <select className="role-select" title="Preview Magnum BI as a lower role"
      value={getViewAs() ?? auth.actual_role} onChange={e => pick(e.target.value)}>
      {options.map(r => <option key={r} value={r}>{r === auth.actual_role ? '' : 'View as '}{ROLE_LABELS[r]}</option>)}
    </select>
  ) : <span className="role-badge">{auth.role_label}</span>
  const limits = auth.restrictions ?? []
  const lock = limits.length > 0 && (
    <span className="limit-badge" title={'Your data is limited to:\n' + limits.map(l => `${l.model_label} › ${l.label}: ${l.values.join(', ') || '(none)'}${l.source === 'organisation' ? ' (organisation)' : ''}`).join('\n')}>
      🔒 Limited data
    </span>
  )
  const name = auth.user?.name || auth.user?.email || 'Signed in'
  return (
    <div className="user-menu" title={`Signed in as ${auth.user?.email} (${auth.role_label})`}>
      {lock}
      {roleBadge}
      <span className="avatar" aria-hidden>{name.trim().charAt(0).toUpperCase()}</span>
      <span className="user-name" title={auth.user?.email ?? undefined}>{name}</span>
      {/* the checklist, back anytime (once left, hidden or done) */}
      {(auth.lite || auth.permissions?.includes('manage_members')) && <NavLink className="link" to="/get-started">Get started</NavLink>}
      <TourButton />
      {/* lite: nobody signs in; its API tokens (for local agents) are still here */}
      <button className="link" onClick={() => setChanging(true)}>{auth.lite ? 'API tokens' : 'Security'}</button>
      {!auth.lite && <button className="link" onClick={signOut}>Sign out</button>}
      {auth.lite && auth.method === 'pin' && <button className="link" onClick={signOut}>Lock</button>}
      {changing && (
        <div className="modal-bg login-overlay">
          <SecurityCard onClose={() => { setChanging(false); refresh() }} />
        </div>
      )}
    </div>
  )
}

/** The welcome tour again, on request. */
function TourButton() {
  const welcome = useWelcomeTour()
  const [none, setNone] = useState(false)
  return <button className="link" data-tour="tour-again" title={none ? 'There is no dashboard to show you around yet' : 'A short tour of a dashboard'}
    onClick={async () => setNone(!(await welcome()))}>Tour</button>
}

/** Where people start: the chat or the dashboards (their choice, or their organisation's). */
function Start() {
  const [home, setHome] = useState<string | null>(null)
  useEffect(() => { api.chat().then(c => setHome(c.home), () => setHome('dashboards')) }, [])
  if (home === null) return null
  return home === 'chat' ? <ChatPage /> : <Home />
}

/** The chat, once it's on (Org admins see it before, to set it up). */
function ChatLink() {
  const can = useCan()
  const [on, setOn] = useState(false)
  useEffect(() => { api.chat().then(c => setOn(c.enabled), () => setOn(false)) }, [])
  return on || can('manage_members') ? <NavLink to="/chat" data-tour={on ? 'nav-chat' : undefined}>Chat</NavLink> : null
}
