import { useEffect, useState } from 'react'
import { api, SIGN_IN_LABELS } from '../api'
import type { Role, SignInMethod, SignInSettings as Settings } from '../api'

const ROLES: [Role, string][] = [['viewer', 'Viewer'], ['user', 'User'], ['power_user', 'Power user'], ['org_admin', 'Org admin']]

/** Org admins: how the organisation's people sign in, within what the platform allows, and the
 *  organisation's own single sign-on provider. */
export default function SignInSettings() {
  const [data, setData] = useState<Settings | null>(null)
  const [methods, setMethods] = useState<SignInMethod[] | null>(null) // null: whatever the platform allows
  const [twoFactor, setTwoFactor] = useState(false)
  const [sso, setSso] = useState(false)
  const [label, setLabel] = useState('')
  const [issuer, setIssuer] = useState('')
  const [clientId, setClientId] = useState('')
  const [secret, setSecret] = useState('')
  const [groupsClaim, setGroupsClaim] = useState('groups')
  const [groups, setGroups] = useState<[string, Role][]>([])
  const [msg, setMsg] = useState<{ kind: 'ok' | 'error'; text: string } | null>(null)
  const [busy, setBusy] = useState(false)

  const adopt = (s: Settings) => {
    setData(s)
    setMethods(s.sign_in.methods ?? null)
    setTwoFactor(Boolean(s.sign_in.require_two_factor))
    setSso(Boolean(s.oidc))
    setLabel(s.oidc?.label ?? '')
    setIssuer(s.oidc?.issuer ?? '')
    setClientId(s.oidc?.client_id ?? '')
    setGroupsClaim(s.oidc?.groups_claim || 'groups')
    setGroups(Object.entries(s.oidc?.group_roles ?? {}))
    setSecret('')
  }
  useEffect(() => { api.signInSettings().then(adopt, e => setMsg({ kind: 'error', text: e.message })) }, [])
  if (!data) return msg ? <div className="error">{msg.text}</div> : null

  const allowed = data.platform.methods
  const chosen = methods ?? allowed
  const toggle = (m: SignInMethod) => setMethods(chosen.includes(m) ? chosen.filter(x => x !== m) : [...chosen, m])
  const save = async () => {
    setBusy(true); setMsg(null)
    try {
      adopt(await api.saveSignInSettings({
        sign_in: { methods: methods && methods.length < allowed.length ? methods : null, require_two_factor: twoFactor },
        oidc: sso ? {
          label, issuer, client_id: clientId, client_secret: secret || undefined, groups_claim: groupsClaim,
          group_roles: Object.fromEntries(groups.filter(([g]) => g.trim())),
        } : null,
      }))
      setMsg({ kind: 'ok', text: 'Saved. People signed in a way that\'s no longer allowed are signed out.' })
    } catch (e) { setMsg({ kind: 'error', text: (e as Error).message }) }
    setBusy(false)
  }

  return (
    <div className="card org-card sign-in-settings">
      <h3>Sign-in</h3>
      <p className="muted small">How people of this organisation sign in. The platform allows:{' '}
        {allowed.map(m => SIGN_IN_LABELS[m]).join(', ') || 'nothing yet'}
        {data.platform.require_two_factor ? '; passwords always need an authenticator code' : ''}.</p>
      <div className="row wrap">
        {allowed.map(m => (
          <label key={m} className="check"><input type="checkbox" checked={chosen.includes(m)} onChange={() => toggle(m)} /> {SIGN_IN_LABELS[m]}</label>
        ))}
      </div>
      {!data.platform.require_two_factor && chosen.includes('password') && (
        <label className="check"><input type="checkbox" checked={twoFactor} onChange={e => setTwoFactor(e.target.checked)} disabled={!data.secret_key} />
          {' '}Passwords need a code from an authenticator app{!data.secret_key && ' (needs MAGNUM_SECRET_KEY on the server)'}</label>
      )}

      {allowed.includes('oidc') && <>
        <label className="check"><input type="checkbox" checked={sso} onChange={e => setSso(e.target.checked)} /> Our own single sign-on provider (OpenID Connect)</label>
        {sso && <>
          <p className="muted small">Microsoft Entra ID, Okta, Keycloak, Auth0, Google Workspace… Register Magnum BI there as a web
            application with this redirect URI: <code className="mono">{data.callback_url}</code>. It signs in addresses
            {' '}{data.domains.map(d => `@${d}`).join(', ') || '(no domains: ask a system owner to add the organisation\'s domains)'} only.</p>
          <div className="grid2">
            <label className="small">Button label<input value={label} onChange={e => setLabel(e.target.value)} placeholder="ACME (Microsoft)" /></label>
            <label className="small">Client id<input value={clientId} onChange={e => setClientId(e.target.value)} /></label>
          </div>
          <label className="small">Issuer <span className="muted">(e.g. https://login.microsoftonline.com/&lt;tenant&gt;/v2.0)</span>
            <input value={issuer} onChange={e => setIssuer(e.target.value)} placeholder="https://" /></label>
          <label className="small">Client secret <span className="muted">{data.oidc?.has_secret ? '(set; leave empty to keep it)' : '(if the provider gives one)'}</span>
            <input type="password" autoComplete="off" value={secret} onChange={e => setSecret(e.target.value)} /></label>
          <label className="small">Groups claim <span className="muted">(the ID token claim with the person's groups)</span>
            <input value={groupsClaim} onChange={e => setGroupsClaim(e.target.value)} /></label>
          <div className="small"><b>Roles from groups</b> <span className="muted">(people in none get the default role; listed members keep their own)</span></div>
          {groups.map(([g, r], i) => (
            <div key={i} className="row">
              <input placeholder="group name or id" value={g} onChange={e => setGroups(groups.map((x, j) => (j === i ? [e.target.value, x[1]] : x)))} />
              <select value={r} onChange={e => setGroups(groups.map((x, j) => (j === i ? [x[0], e.target.value as Role] : x)))}>
                {ROLES.map(([id, l]) => <option key={id} value={id}>{l}</option>)}
              </select>
              <button type="button" className="icon" onClick={() => setGroups(groups.filter((_, j) => j !== i))}>✕</button>
            </div>
          ))}
          <button type="button" className="link" onClick={() => setGroups([...groups, ['', 'viewer']])}>+ group</button>
        </>}
      </>}
      {msg && <div className={msg.kind}>{msg.text}</div>}
      <div className="row"><button className="primary" disabled={busy} onClick={save}>{busy ? 'Saving…' : 'Save sign-in settings'}</button></div>
    </div>
  )
}
