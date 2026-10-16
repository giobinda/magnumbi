import { createContext, useCallback, useContext, useEffect, useState } from 'react'
import type { FormEvent, ReactNode } from 'react'
import { AUTH_REQUIRED_EVENT, api, setOrg, setViewAs } from './api'
import type { AuthState, Permission } from './api'
import LoginPage from './pages/LoginPage'
import { TwoFactorSetup } from './components/Security'

type Ctx = { auth: AuthState | null; refresh: () => Promise<void>; signOut: () => Promise<void> }
const AuthContext = createContext<Ctx>({ auth: null, refresh: async () => {}, signOut: async () => {} })
export const useAuth = () => useContext(AuthContext)

/** UI convenience only: the server enforces every permission on its own. */
export function useCan() {
  const { auth } = useAuth()
  const perms = new Set(auth?.permissions ?? [])
  return (p: Permission) => perms.has(p)
}

export const Brand = ({ big }: { big?: boolean }) => <div className={big ? 'brand big' : 'brand'}>Magnum<span>BI</span></div>

/** Shows the login screen until the user is signed in, and again whenever the API reports that
 *  the sign-in expired. A one-time password must be replaced before anything else. */
export function AuthGate({ children }: { children: ReactNode }) {
  const [auth, setAuth] = useState<AuthState | null>(null)
  const [error, setError] = useState<string | null>(null)

  const refresh = useCallback(async () => {
    try { setAuth(await api.me()); setError(null) } catch (e) { setError((e as Error).message) }
  }, [])
  const signOut = useCallback(async () => {
    await api.logout().catch(() => {})
    setViewAs(null)  // per-browser choices ("view as", organisation) never outlive the sign-in
    setOrg(null)
    await refresh()
  }, [refresh])

  useEffect(() => {
    refresh()
    const onRequired = () => { refresh() }
    // embedded dashboards sign in with Google through a popup, which reports back here
    const onMessage = (e: MessageEvent) => { if (e.origin === location.origin && e.data?.type === 'magnum:login') refresh() }
    window.addEventListener(AUTH_REQUIRED_EVENT, onRequired)
    window.addEventListener('message', onMessage)
    return () => { window.removeEventListener(AUTH_REQUIRED_EVENT, onRequired); window.removeEventListener('message', onMessage) }
  }, [refresh])

  if (!auth) return error ? <div className="page"><div className="error">Cannot reach the Magnum BI server: {error}</div></div> : null
  const needLogin = auth.required && !auth.authenticated
  return (
    <AuthContext.Provider value={{ auth, refresh, signOut }}>
      {needLogin || location.pathname === '/login' ? <LoginPage auth={auth} onSignedIn={refresh} />
        : auth.must_change_password ? <ChangePasswordCard required onDone={refresh} onCancel={signOut} />
        : auth.two_factor_setup ? <TwoFactorSetup required onDone={refresh} onCancel={signOut} />
        : auth.member === false ? <NotAMember name={auth.user?.email || auth.user?.name} signOut={signOut} />
        : children}
    </AuthContext.Provider>
  )
}

/** Set or change your own password. `required`: after a one-time password from an admin. */
export function ChangePasswordCard({ required, hasPassword = true, onDone, onCancel }: {
  required?: boolean; hasPassword?: boolean; onDone: () => void; onCancel: () => void
}) {
  const [current, setCurrent] = useState('')
  const [next, setNext] = useState('')
  const [again, setAgain] = useState('')
  const [busy, setBusy] = useState(false)
  const [err, setErr] = useState<string | null>(null)
  const submit = async (e: FormEvent) => {
    e.preventDefault()
    if (next !== again) { setErr("The new passwords don't match."); return }
    setBusy(true); setErr(null)
    try { await api.changePassword(hasPassword ? current : null, next); onDone() }
    catch (x) { setErr((x as Error).message) }
    setBusy(false)
  }
  return (
    <div className="login-wrap">
      <form className="login-card" onSubmit={submit}>
        <Brand big />
        <h1>{required ? 'Choose your password' : hasPassword ? 'Change password' : 'Set a password'}</h1>
        {required && <p className="muted">You signed in with a one-time password. Choose your own to continue.</p>}
        {hasPassword && (
          <label>{required ? 'One-time password' : 'Current password'}
            <input type="password" autoComplete="current-password" value={current} onChange={e => setCurrent(e.target.value)} required />
          </label>
        )}
        <label>New password
          <input type="password" autoComplete="new-password" minLength={10} value={next} onChange={e => setNext(e.target.value)} required />
        </label>
        <label>New password, again
          <input type="password" autoComplete="new-password" value={again} onChange={e => setAgain(e.target.value)} required />
        </label>
        <p className="muted small">At least 10 characters. A few unrelated words make a strong, memorable password.</p>
        {err && <div className="error">{err}</div>}
        <button className="primary login-btn" disabled={busy || !next}>{busy ? 'Saving…' : 'Save password'}</button>
        <button type="button" className="link" onClick={onCancel}>{required ? 'Sign out' : 'Cancel'}</button>
      </form>
    </div>
  )
}

/** Signed in, but no organisation has this person (the API refuses everything anyway). */
function NotAMember({ name, signOut }: { name?: string; signOut: () => void }) {
  return (
    <div className="login-wrap">
      <div className="login-card">
        <Brand big />
        <h1>No organisation yet</h1>
        <p className="muted">You're signed in{name ? ` as ${name}` : ''}, but you aren't a member of any organisation on
          Magnum BI. Ask your company's Magnum BI admin to add you.</p>
        <button className="primary" onClick={signOut}>Sign out</button>
      </div>
    </div>
  )
}
