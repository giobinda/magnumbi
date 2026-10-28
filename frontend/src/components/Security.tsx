import { useEffect, useState } from 'react'
import type { FormEvent } from 'react'
import { api } from '../api'
import type { ApiToken, PasskeyInfo } from '../api'
import AgentSetup from './AgentSetup'
import { Brand, ChangePasswordCard, useAuth } from '../auth'
import { createPasskey, passkeysSupported } from '../webauthn'

/** Set up an authenticator app: scan, confirm with a code, keep the recovery codes.
 *  `required`: the organisation now requires one (nothing else is possible until it's done). */
export function TwoFactorSetup({ required, onDone, onCancel }: { required?: boolean; onDone: () => void; onCancel: () => void }) {
  const [setup, setSetup] = useState<{ secret: string; qr_svg: string } | null>(null)
  const [code, setCode] = useState('')
  const [recovery, setRecovery] = useState<string[] | null>(null)
  const [busy, setBusy] = useState(false)
  const [err, setErr] = useState<string | null>(null)
  useEffect(() => { api.twoFactorSetup().then(setSetup, e => setErr(e.message)) }, [])

  const confirm = async (e: FormEvent) => {
    e.preventDefault()
    setBusy(true); setErr(null)
    try { setRecovery((await api.twoFactorEnable(code)).recovery_codes) } catch (x) { setErr((x as Error).message) }
    setBusy(false)
  }
  const download = () => {
    const blob = new Blob([`Magnum BI recovery codes (each works once)\n\n${recovery!.join('\n')}\n`], { type: 'text/plain' })
    const a = Object.assign(document.createElement('a'), { href: URL.createObjectURL(blob), download: 'magnum-bi-recovery-codes.txt' })
    a.click()
    URL.revokeObjectURL(a.href)
  }

  return (
    <div className="login-wrap">
      <div className="login-card">
        <Brand big />
        <h1>{recovery ? 'Keep your recovery codes' : 'Set up an authenticator app'}</h1>
        {required && !recovery && <p className="muted">Your organisation requires a code from an authenticator app when you sign in
          with a password. Set one up to continue.</p>}
        {err && <div className="error">{err}</div>}
        {recovery ? (
          <>
            <p className="muted">If you lose your phone, each of these codes signs you in once instead of an app code. They won't be
              shown again: download or print them now.</p>
            <ul className="recovery-codes">{recovery.map(c => <li key={c}><code>{c}</code></li>)}</ul>
            <button type="button" onClick={download}>Download</button>
            <button type="button" className="primary login-btn" onClick={onDone}>Done</button>
          </>
        ) : setup ? (
          <form className="login-form" onSubmit={confirm}>
            <p className="muted small">Scan this with an authenticator app (Google Authenticator, Microsoft Authenticator, 1Password,
              …), then enter the 6-digit code it shows.</p>
            <img className="totp-qr" alt="QR code for your authenticator app" src={`data:image/svg+xml;utf8,${encodeURIComponent(setup.qr_svg)}`} />
            <details className="small"><summary>Can't scan it?</summary>Enter this key in the app: <code className="mono">{setup.secret}</code></details>
            <label>Code from the app
              <input inputMode="numeric" autoComplete="one-time-code" value={code} onChange={e => setCode(e.target.value)} required autoFocus />
            </label>
            <button className="primary login-btn" disabled={busy || !code}>{busy ? 'Checking…' : 'Turn on'}</button>
          </form>
        ) : !err && <p className="muted">One moment…</p>}
        {!recovery && <button type="button" className="link" onClick={onCancel}>{required ? 'Sign out' : 'Cancel'}</button>}
      </div>
    </div>
  )
}

/** Your sign-in security: password, authenticator app, passkeys. */
export function SecurityCard({ onClose }: { onClose: () => void }) {
  const { auth, refresh } = useAuth()
  const [view, setView] = useState<'main' | 'password' | 'two-factor'>('main')
  const [keys, setKeys] = useState<PasskeyInfo[] | null>(null)
  const [keysAvailable, setKeysAvailable] = useState(false)
  const [name, setName] = useState('')
  const [offPassword, setOffPassword] = useState<string | null>(null)
  const [msg, setMsg] = useState<{ kind: 'ok' | 'error'; text: string } | null>(null)
  const loadKeys = () => api.passkeys().then(r => { setKeys(r.passkeys); setKeysAvailable(r.available) }, () => setKeys([]))
  useEffect(() => { loadKeys() }, [])
  if (!auth) return null
  const fail = (x: unknown) => setMsg({ kind: 'error', text: (x as Error).name === 'NotAllowedError' ? 'Cancelled.' : (x as Error).message })

  if (view === 'password') {
    return <ChangePasswordCard hasPassword={auth.has_password} onDone={() => { setView('main'); refresh() }} onCancel={() => setView('main')} />
  }
  if (view === 'two-factor') {
    return <TwoFactorSetup onDone={() => { setView('main'); refresh() }} onCancel={() => setView('main')} />
  }

  const addKey = async () => {
    setMsg(null)
    try {
      const { ticket, options } = await api.passkeyRegisterStart()
      await api.passkeyRegisterFinish(ticket, name || 'Passkey', await createPasskey(options))
      setName('')
      setMsg({ kind: 'ok', text: 'Passkey added.' })
      loadKeys()
    } catch (x) { fail(x) }
  }
  const removeKey = async (k: PasskeyInfo) => {
    if (!confirm(`Remove the passkey "${k.name}"? It won't sign you in any more.`)) return
    try { await api.deletePasskey(k.id); loadKeys() } catch (x) { fail(x) }
  }
  const switchOff = async (e: FormEvent) => {
    e.preventDefault()
    try { await api.twoFactorDisable(offPassword ?? ''); setOffPassword(null); setMsg({ kind: 'ok', text: 'Authenticator switched off.' }); refresh() }
    catch (x) { fail(x) }
  }

  return (
    <div className="login-wrap">
      <div className="login-card security-card">
        <h1>{auth.lite ? 'API tokens' : 'Sign-in security'}</h1>
        <p className="muted small">{auth.user?.email}</p>
        {msg && <div className={msg.kind}>{msg.text}</div>}

        {/* lite: nobody signs in, so no password, authenticator or passkeys */}
        {!auth.lite && <>
        <section>
          <h3>Password</h3>
          <p className="muted small">{auth.has_password ? 'Set.' : 'None: you sign in another way.'}</p>
          {auth.methods.password && <button type="button" onClick={() => setView('password')}>{auth.has_password ? 'Change password' : 'Set a password'}</button>}
        </section>

        {auth.has_password && (
          <section>
            <h3>Authenticator app</h3>
            <p className="muted small">{auth.two_factor
              ? 'On: signing in with your password also asks for a code from your app.'
              : 'Off. With it on, a stolen password alone isn\'t enough to sign in.'}</p>
            {!auth.two_factor && <button type="button" onClick={() => setView('two-factor')}>Set up</button>}
            {auth.two_factor && offPassword === null && <button type="button" className="link danger" onClick={() => setOffPassword('')}>Switch off…</button>}
            {auth.two_factor && offPassword !== null && (
              <form className="row" onSubmit={switchOff}>
                <input type="password" placeholder="Your password" autoComplete="current-password" value={offPassword} onChange={e => setOffPassword(e.target.value)} required />
                <button className="danger">Switch off</button>
                <button type="button" className="link" onClick={() => setOffPassword(null)}>Cancel</button>
              </form>
            )}
          </section>
        )}

        {keysAvailable && (
          <section>
            <h3>Passkeys</h3>
            <p className="muted small">Sign in with your fingerprint, face or device PIN, or a security key: nothing to type, nothing to steal.</p>
            {keys?.length ? (
              <ul className="passkey-list">{keys.map(k => (
                <li key={k.id}>
                  <span><b>{k.name}</b> <span className="muted small">added {new Date(k.created_at).toLocaleDateString()}
                    {k.last_used_at ? `, last used ${new Date(k.last_used_at).toLocaleDateString()}` : ''}</span></span>
                  <button type="button" className="link danger" onClick={() => removeKey(k)}>Remove</button>
                </li>
              ))}</ul>
            ) : <p className="muted small">None yet.</p>}
            {passkeysSupported() ? (
              <div className="row">
                <input placeholder="Name, e.g. Work laptop" value={name} onChange={e => setName(e.target.value)} maxLength={60} />
                <button type="button" onClick={addKey}>Add a passkey</button>
              </div>
            ) : <p className="muted small">This browser doesn't support passkeys.</p>}
          </section>
        )}
        </>}
        <ApiTokens />
        <section>
          <h3>Connect an AI agent</h3>
          <p className="muted small">Claude, Codex or Cursor explore your data and build dashboards for you, through MCP.</p>
          <AgentSetup />
        </section>
        <button type="button" className="primary" onClick={onClose}>Close</button>
      </div>
    </div>
  )
}

/** Tokens for agents and scripts (MCP clients, CI, notebooks): they act for you, in this
 *  organisation, with at most your role. The secret is shown once. */
function ApiTokens() {
  const [tokens, setTokens] = useState<ApiToken[] | null>(null)
  const [name, setName] = useState('')
  const [scope, setScope] = useState<'read' | 'write'>('read')
  const [days, setDays] = useState(90)
  const [created, setCreated] = useState<string | null>(null)
  const [err, setErr] = useState<string | null>(null)
  // live ones only: not revoked, not expired
  const load = () => api.tokens().then(r => setTokens(r.tokens.filter(t => !t.revoked_at && Date.parse(t.expires_at) > Date.now())),
    e => setErr(e.message))
  useEffect(() => { load() }, [])
  if (!tokens) return err ? <section><h3>API tokens</h3><p className="muted small">{err}</p></section> : null

  const create = async () => {
    setErr(null)
    try {
      const t = await api.createToken({ name, scope, expires_in_days: days })
      setCreated(t.token); setName(''); load()
    } catch (e) { setErr((e as Error).message) }
  }
  const revoke = async (t: ApiToken) => {
    if (!confirm(`Revoke "${t.name}"? Anything using it stops working.`)) return
    try { await api.revokeToken(t.id); load() } catch (e) { setErr((e as Error).message) }
  }
  return (
    <section>
      <h3>API tokens</h3>
      <p className="muted small">For agents and scripts: send it as <code>Authorization: Bearer …</code>. A token acts for you,
        in this organisation, and can do no more than you. Read-only tokens can read and run queries but change nothing.</p>
      {created && (
        <div className="ok token-once">Copy it now, it won't be shown again:<br /><code className="mono">{created}</code>
          <div className="row"><button type="button" onClick={() => navigator.clipboard?.writeText(created)}>Copy</button>
            <button type="button" className="link" onClick={() => setCreated(null)}>Done</button></div>
          <McpSetup token={created} />
        </div>
      )}
      {tokens.length > 0 ? (
        <ul className="passkey-list">{tokens.map(t => (
          <li key={t.id}>
            <span><b>{t.name}</b> <span className="muted small">{t.scope === 'write' ? 'read & write' : 'read-only'} · expires{' '}
              {new Date(t.expires_at).toLocaleDateString()}{t.last_used_at ? ` · last used ${new Date(t.last_used_at).toLocaleDateString()}` : ' · never used'}</span></span>
            <button type="button" className="link danger" onClick={() => revoke(t)}>Revoke</button>
          </li>
        ))}</ul>
      ) : <p className="muted small">None.</p>}
      <div className="row wrap">
        <input placeholder="What will use it? e.g. Claude Desktop" value={name} onChange={e => setName(e.target.value)} maxLength={60} />
        <select value={scope} onChange={e => setScope(e.target.value as 'read' | 'write')}>
          <option value="read">Read-only</option><option value="write">Read & write</option>
        </select>
        <select value={days} onChange={e => setDays(Number(e.target.value))}>
          {[7, 30, 90, 365].map(d => <option key={d} value={d}>{d} days</option>)}
        </select>
        <button type="button" disabled={!name.trim()} onClick={create}>Create token</button>
      </div>
      {err && <div className="error">{err}</div>}
    </section>
  )
}

/** How to connect an MCP client (Claude Code, Cursor, Claude Desktop) with a new token. */
function McpSetup({ token }: { token: string }) {
  const url = `${location.origin}/api/mcp`
  const snippets: [string, string][] = [
    ['Claude Code', `claude mcp add --transport http magnum-bi ${url} --header "Authorization: Bearer ${token}"`],
    ['Cursor (mcp.json)', JSON.stringify({ mcpServers: { 'magnum-bi': { url, headers: { Authorization: `Bearer ${token}` } } } }, null, 2)],
    ['Claude Desktop (claude_desktop_config.json)', JSON.stringify({ mcpServers: { 'magnum-bi': { command: 'npx',
      args: ['-y', 'mcp-remote', url, '--header', `Authorization: Bearer ${token}`] } } }, null, 2)],
  ]
  return (
    <details className="small mcp-setup">
      <summary>Connect an AI agent (MCP) with it</summary>
      <p>Magnum BI's MCP endpoint: <code className="mono">{url}</code>. The agent can then explore models, run queries and, with a
        read &amp; write token, edit dashboards, blends and members, always as you.</p>
      {snippets.map(([label, text]) => (
        <div key={label}><b>{label}</b><pre className="mono">{text}</pre></div>
      ))}
    </details>
  )
}
