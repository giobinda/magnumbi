import { useEffect, useState } from 'react'
import type { FormEvent } from 'react'
import { ApiError, api } from '../api'
import type { AuthState, SignInMethods } from '../api'
import { Brand } from '../auth'
import { passkeysSupported, signWithPasskey } from '../webauthn'

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

/** Signed in: go where the person was heading (or refresh the app in place). */
function done(onSignedIn: () => void) {
  if (location.pathname === '/login') location.replace(nextUrl())
  else onSignedIn()
}

/** Sign-in, email first: then the ways this person's organisation allows (password, passkey, its
 *  own single sign-on, Google), and an authenticator code when they use one. */
export default function LoginPage({ auth, onSignedIn }: { auth: AuthState; onSignedIn: () => void }) {
  const error = new URLSearchParams(location.search).get('error')
  const [email, setEmail] = useState('')
  const [ways, setWays] = useState<SignInMethods | null>(null)
  const [ticket, setTicket] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [err, setErr] = useState<string | null>(null)

  // already signed in on /login: go where the user was heading (or, in the sign-in popup of an
  // embedded dashboard, tell the dashboard and close)
  useEffect(() => {
    if (location.pathname !== '/login' || !auth.authenticated) return
    if (new URLSearchParams(location.search).get('popup') && window.opener) {
      window.opener.postMessage({ type: 'magnum:login' }, location.origin)
      window.close()
      return
    }
    location.replace(nextUrl())
  }, [auth])

  const lookUp = async (e: FormEvent) => {
    e.preventDefault()
    setBusy(true); setErr(null)
    try { setWays(await api.signInMethods(email)) } catch (x) { setErr((x as Error).message) }
    setBusy(false)
  }
  const back = () => { setWays(null); setTicket(null); setErr(null) }

  return (
    <div className="login-wrap">
      <div className="login-card">
        <Brand big />
        <h1>Sign in</h1>
        {error && !ways && <div className="error">{error}</div>}
        {!ways ? (
          <form className="login-form" onSubmit={lookUp}>
            <label>Email
              <input type="email" autoComplete="username webauthn" value={email} onChange={e => setEmail(e.target.value)} required autoFocus />
            </label>
            {err && <div className="error">{err}</div>}
            <button className="primary login-btn" disabled={busy || !email}>{busy ? 'One moment…' : 'Continue'}</button>
          </form>
        ) : ticket ? (
          <CodeForm ticket={ticket} onSignedIn={onSignedIn} onBack={back} />
        ) : (
          <div className="login-form">
            <div className="login-who"><span>{email}</span> <button type="button" className="link" onClick={back}>Change</button></div>
            {ways.methods.length === 0 && <div className="warn">This account can't sign in here at the moment. Ask your administrator.</div>}
            {ways.oidc && <ProviderButton label={ways.oidc.label} url={ways.oidc.url} onSignedIn={onSignedIn} />}
            {ways.methods.includes('passkey') && passkeysSupported() && <PasskeyButton email={email} onSignedIn={onSignedIn} />}
            {ways.methods.includes('password') && (ways.oidc || ways.methods.includes('passkey')) && <div className="login-or"><span>or</span></div>}
            {ways.methods.includes('password') && <PasswordForm email={email} onSignedIn={onSignedIn} onCode={setTicket} />}
            {ways.methods.includes('google') && <ProviderButton label="Google" url="/api/auth/google/login" onSignedIn={onSignedIn} />}
          </div>
        )}
      </div>
    </div>
  )
}

function PasswordForm({ email, onSignedIn, onCode }: { email: string; onSignedIn: () => void; onCode: (ticket: string) => void }) {
  const [password, setPassword] = useState('')
  const [busy, setBusy] = useState(false)
  const [err, setErr] = useState<string | null>(null)
  const submit = async (e: FormEvent) => {
    e.preventDefault()
    setBusy(true); setErr(null)
    try {
      const r = await api.login(email, password)
      setPassword('')
      if (r.two_factor && r.ticket) onCode(r.ticket)
      else done(onSignedIn)
    } catch (x) {
      setErr(x instanceof ApiError && x.status === 401 ? 'Wrong email or password.' : (x as Error).message)
    }
    setBusy(false)
  }
  return (
    <form onSubmit={submit} className="login-form">
      <label>Password
        <input type="password" autoComplete="current-password" value={password} onChange={e => setPassword(e.target.value)} required autoFocus />
      </label>
      {err && <div className="error">{err}</div>}
      <button className="primary login-btn" disabled={busy || !password}>{busy ? 'Signing in…' : 'Sign in'}</button>
      <p className="muted small">Forgot your password? Ask your organisation's Magnum BI admin for a one-time password.</p>
    </form>
  )
}

/** The authenticator code (or a recovery code) after the password. */
function CodeForm({ ticket, onSignedIn, onBack }: { ticket: string; onSignedIn: () => void; onBack: () => void }) {
  const [code, setCode] = useState('')
  const [busy, setBusy] = useState(false)
  const [err, setErr] = useState<string | null>(null)
  const submit = async (e: FormEvent) => {
    e.preventDefault()
    setBusy(true); setErr(null)
    try { await api.loginTwoFactor(ticket, code); done(onSignedIn) }
    catch (x) { setErr((x as Error).message); setCode('') }
    setBusy(false)
  }
  return (
    <form onSubmit={submit} className="login-form">
      <label>Code from your authenticator app
        <input inputMode="numeric" autoComplete="one-time-code" value={code} onChange={e => setCode(e.target.value)} required autoFocus />
      </label>
      <p className="muted small">Lost your phone? Enter one of your recovery codes instead.</p>
      {err && <div className="error">{err}</div>}
      <button className="primary login-btn" disabled={busy || !code}>{busy ? 'Checking…' : 'Sign in'}</button>
      <button type="button" className="link" onClick={onBack}>Start again</button>
    </form>
  )
}

function PasskeyButton({ email, onSignedIn }: { email: string; onSignedIn: () => void }) {
  const [busy, setBusy] = useState(false)
  const [err, setErr] = useState<string | null>(null)
  const go = async () => {
    setBusy(true); setErr(null)
    try {
      const { ticket, options } = await api.passkeyLoginStart(email)
      await api.passkeyLoginFinish(ticket, await signWithPasskey(options))
      done(onSignedIn)
    } catch (x) {
      setErr((x as Error).name === 'NotAllowedError' ? 'The passkey request was cancelled.' : (x as Error).message)
    }
    setBusy(false)
  }
  return <>
    <button type="button" className="primary login-btn" onClick={go} disabled={busy}>{busy ? 'Waiting for your passkey…' : 'Sign in with a passkey'}</button>
    {err && <div className="error">{err}</div>}
  </>
}

/** Google, or the organisation's own provider: off to it (in a popup inside an embedded dashboard,
 *  since providers refuse to load in iframes). */
function ProviderButton({ label, url, onSignedIn }: { label: string; url: string; onSignedIn: () => void }) {
  const [waiting, setWaiting] = useState(false)
  const start = () => {
    if (!embedded) { location.href = `${url}?next=${encodeURIComponent(nextUrl())}`; return }
    const w = window.open(`${url}?next=${encodeURIComponent('/login?popup=1')}`, 'magnum-login', 'width=520,height=680')
    if (!w) { location.href = url; return }
    setWaiting(true)
    const t = setInterval(() => { if (w.closed) { clearInterval(t); setWaiting(false); onSignedIn() } }, 500)
  }
  return (
    <button type="button" className="login-btn" onClick={start} disabled={waiting}>
      {waiting ? 'Waiting for sign-in window…' : `Sign in with ${label}`}
    </button>
  )
}
