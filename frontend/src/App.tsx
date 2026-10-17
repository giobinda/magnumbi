import { useState } from 'react'
import { BrowserRouter, NavLink, Route, Routes } from 'react-router-dom'
import { AuthGate, useAuth, useCan } from './auth'
import { SecurityCard } from './components/Security'
import { getOrg, getViewAs, setOrg, setViewAs } from './api'
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
import ThemesPage from './pages/ThemesPage'
import ModelsPage from './pages/ModelsPage'

const ROLE_LABELS: Record<string, string> = { viewer: 'Viewer', user: 'User', power_user: 'Power user', org_admin: 'Org admin', system_owner: 'System owner' }

export default function App() {
  return (
    <BrowserRouter>
      <Routes>
        {/* public links: no sign-in */}
        <Route path="/s/:org/:token" element={<PublicPage />} />
        <Route path="/g/:org/:token" element={<GuestPage />} />
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
            <div className="app">
              <nav className="topnav">
                <NavLink to="/" className="brand">Magnum<span>BI</span></NavLink>
                <NavLink to="/" end>Dashboards</NavLink>
                <Guarded perm="explore"><NavLink to="/explore">Explore</NavLink></Guarded>
                <Guarded perm="explore"><NavLink to="/blends">Blends</NavLink></Guarded>
                <MembersLink />
                <Guarded perm="manage_members"><NavLink to="/connections">Connections</NavLink></Guarded>
                <OrgOnly><Guarded perm="manage_members"><NavLink to="/models">Models</NavLink></Guarded></OrgOnly>
                <Guarded perm="manage_members"><NavLink to="/themes">Themes</NavLink></Guarded>
                <Guarded perm="manage_members"><NavLink to="/usage">Usage</NavLink></Guarded>
                <Guarded perm="manage_orgs"><NavLink to="/organisations">Organisations</NavLink></Guarded>
                <div className="nav-right"><OrgSwitcher /><ThemePicker /><UserMenu /></div>
              </nav>
              <Routes>
                <Route path="/" element={<Home />} />
                <Route path="/explore" element={<Guarded perm="explore" page><ExplorePage /></Guarded>} />
                <Route path="/blends" element={<Guarded perm="explore" page><BlendsPage /></Guarded>} />
                <Route path="/themes" element={<Guarded perm="manage_members" page><ThemesPage /></Guarded>} />
                <Route path="/connections" element={<Guarded perm="manage_members" page><ConnectionsPage /></Guarded>} />
                <Route path="/models" element={<Guarded perm="manage_members" page><ModelsPage /></Guarded>} />
                <Route path="/usage" element={<Guarded perm="manage_members" page><UsagePage /></Guarded>} />
                <Route path="/members" element={<Guarded perm="manage_members" page><MembersPage /></Guarded>} />
                <Route path="/organisations" element={<Guarded perm="manage_orgs" page><OrganisationsPage /></Guarded>} />
                <Route path="/dashboards/:id" element={<DashboardPage />} />
              </Routes>
              <Credit />
            </div>
          } />
        </Routes>
      </AuthGate>
  )
}

/** Attribution required by the license's additional terms (NOTICE): keep it visible in forks. */
function Credit() {
  return <footer className="app-credit">Powered by Magnum BI · © Giovanni Andrea Binda · AGPL-3.0</footer>
}

/** Members are managed inside an organisation (system owners: after choosing one). */
/** Only inside an organisation (not the platform workspace). */
function OrgOnly({ children }: { children: React.ReactNode }) {
  const { auth } = useAuth()
  return auth?.org ? <>{children}</> : null
}

function MembersLink() {
  const { auth } = useAuth()
  return auth?.org ? <Guarded perm="manage_members"><NavLink to="/members">Members</NavLink></Guarded> : null
}

/** System owners: work in the platform workspace (templates, every model) or inside an organisation. */
function OrgSwitcher() {
  const { auth } = useAuth()
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
  const roleBadge = options.length > 1 ? (
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
      <button className="link" onClick={() => setChanging(true)}>Security</button>
      <button className="link" onClick={signOut}>Sign out</button>
      {changing && (
        <div className="modal-bg login-overlay">
          <SecurityCard onClose={() => { setChanging(false); refresh() }} />
        </div>
      )}
    </div>
  )
}
