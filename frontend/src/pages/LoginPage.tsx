import { useEffect, useState } from 'react'
import type { FormEvent } from 'react'
import { ApiError, api } from '../api'
import type { AuthState } from '../api'
import { Brand } from '../auth'

const embedded = window.self !== window.top

/** Where to go after sign-in: only a path on this site (never javascript:, //host or another origin). */
function safeNext(n: string | null): string {
  if (!n || !n.startsWith('/') || n.startsWith('//') || n.includes('\\')) return '/'
  try {
    const u = new URL(n, location.origin)
    return u.origin === location.origin ? u.pathname + u.search + u.hash : '/'
  } catch { return '/' }
}

function nextUrl() {
  const params = new URLSearchParams(location.search)
  if (location.pathname === '/login') return safeNext(params.get('next'))
  return location.pathname + location.search + location.hash
}

export default function LoginPage({ auth, onSignedIn }: { auth: AuthState; onSignedIn: () => void }) {
  const error = new URLSearchParams(location.search).get('error')

  // already signed in on /login: go where the user was heading (or, in the Google sign-in popup
  // of an embedded dashboard, tell the dashboard and close)
  useEffect(() => {
    if (location.pathname !== '/login' || !auth.authenticated) return
    if (new URLSearchParams(location.search).get('popup') && window.opener) {
      window.opener.postMessage({ type: 'magnum:login' }, location.origin)
      window.close()
      return
    }
    location.replace(nextUrl())
  }, [auth])

  const { password, google } = auth.methods
  return (
    <div className="login-wrap">
      <div className="login-card">
        <Brand big />
        <h1>Sign in</h1>
        {error && <div className="error">{error}</div>}
        {password && <PasswordForm onSignedIn={onSignedIn} />}
        {password && google && <div className="login-or"><span>or</span></div>}
        {google && <GoogleButton onSignedIn={onSignedIn} />}
        {!password && !google && <div className="warn">No sign-in method is configured on this server.</div>}
      </div>
    </div>
  )
}

function PasswordForm({ onSignedIn }: { onSignedIn: () => void }) {
  const [email, setEmail] = useState('')
  const [password, setPassword] = useState('')
  const [busy, setBusy] = useState(false)
  const [err, setErr] = useState<string | null>(null)
  const submit = async (e: FormEvent) => {
    e.preventDefault()
    setBusy(true); setErr(null)
    try {
      await api.login(email, password)
      setPassword('')
      if (location.pathname === '/login') location.replace(nextUrl())
      else onSignedIn()
    } catch (x) {
      setErr(x instanceof ApiError && x.status === 401 ? 'Wrong email or password.' : (x as Error).message)
    }
    setBusy(false)
  }
  return (
    <form className="login-form" onSubmit={submit}>
      <label>Email
        <input type="email" autoComplete="username" value={email} onChange={e => setEmail(e.target.value)} required autoFocus />
      </label>
      <label>Password
        <input type="password" autoComplete="current-password" value={password} onChange={e => setPassword(e.target.value)} required />
      </label>
      {err && <div className="error">{err}</div>}
      <button className="primary login-btn" disabled={busy || !email || !password}>{busy ? 'Signing in…' : 'Sign in'}</button>
      <p className="muted small">Forgot your password? Ask your organisation's Magnum BI admin for a one-time password.</p>
    </form>
  )
}

function GoogleButton({ onSignedIn }: { onSignedIn: () => void }) {
  const [waiting, setWaiting] = useState(false)
  const start = () => {
    if (!embedded) { location.href = `/api/auth/google/login?next=${encodeURIComponent(nextUrl())}`; return }
    // Google refuses to load inside an iframe: sign in in a popup, which reports back
    const w = window.open(`/api/auth/google/login?next=${encodeURIComponent('/login?popup=1')}`, 'magnum-login',
      'width=520,height=680')
    if (!w) { location.href = '/api/auth/google/login'; return }
    setWaiting(true)
    const t = setInterval(() => { if (w.closed) { clearInterval(t); setWaiting(false); onSignedIn() } }, 500)
  }
  return (
    <button className="login-btn" onClick={start} disabled={waiting}>
      {waiting ? 'Waiting for sign-in window…' : 'Sign in with Google'}
    </button>
  )
}
