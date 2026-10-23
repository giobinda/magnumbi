import { useEffect, useState } from 'react'
import { api } from '../api'
import { folderFiles, planSummary, takesSecret } from '../code'
import type { Plan } from '../code'
import { saveFile } from '../download'

const SIGN: Record<string, string> = { create: '+', update: '~', delete: '−', error: '!' }

/** Org admins: the organisation's setup as YAML files to keep in git, and files applied back after a plan. */
export default function CodePage() {
  const [count, setCount] = useState<number | null>(null)
  const [files, setFiles] = useState<Record<string, string> | null>(null)
  const [folder, setFolder] = useState('')
  const [secrets, setSecrets] = useState<Record<string, string>>({})
  const [prune, setPrune] = useState(false)
  const [plan, setPlan] = useState<Plan | null>(null)
  const [busy, setBusy] = useState(false)
  const [done, setDone] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)

  const load = () => api.code().then(r => setCount(Object.keys(r.files).length), e => setError((e as Error).message))
  useEffect(() => { load() }, [])

  const download = async () => {
    try { const { blob, name } = await api.codeZip(); saveFile(blob, name) } catch (e) { setError((e as Error).message) }
  }

  const choose = async (list: FileList | null) => {
    setPlan(null); setDone(null); setError(null); setSecrets({})
    if (!list?.length) return
    const read = await Promise.all(Array.from(list).map(async f => ({ path: f.webkitRelativePath || f.name, text: await f.text() })))
    const chosen = folderFiles(read)
    setFolder(read[0].path.split('/')[0]); setFiles(chosen)
    if (!Object.keys(chosen).length) setError('No YAML files in that folder.')
    else run(chosen, {}, prune, false)
  }

  const run = async (f: Record<string, string>, s: Record<string, string>, p: boolean, apply: boolean) => {
    setBusy(true); setError(null); setDone(null)
    const given = Object.fromEntries(Object.entries(s).filter(([, v]) => v))
    try {
      const r = await api.applyCode(f, given, p, apply)
      if (apply) { setDone(`Applied: ${planSummary(r)}.`); setPlan(null); setSecrets({}); load() } else setPlan(r)
    } catch (e) { setError((e as Error).message) } finally { setBusy(false) }
  }

  const toApply = plan?.changes.length ?? 0
  return (
    <div className="page narrow code-page">
      <h1>Setup as code</h1>
      <p className="muted">Everything your organisation sets up here can be kept as YAML files in git, reviewed, and applied back,
        here or with <code>magnum-bi code</code>. Applying shows a plan first and goes through the same checks as the app.
        Passwords, keys and webhook addresses are never in the files.</p>
      {error && <div className="error">{error}</div>}

      <h2 className="section">Download</h2>
      <p>{count === null ? '…' : `${count} files: connections, models, formulas, blends, members, sign-in rules, themes, glossary, catalog, dashboards, delivery, chat, reports.`}</p>
      <button onClick={download} disabled={!count}>Download as a zip</button>

      <h2 className="section">Apply files</h2>
      <label>A folder of files (as downloaded, unzipped)
        {/* webkitdirectory: choose a whole folder */}
        <input type="file" {...{ webkitdirectory: '' }} multiple onChange={e => choose(e.target.files)} />
      </label>
      <label className="check"><input type="checkbox" checked={prune} onChange={e => { setPrune(e.target.checked); if (files) run(files, secrets, e.target.checked, false) }} />
        Also delete what the files don't have (never the members or the sign-in rules)</label>

      {plan && files && (
        <div className="code-plan">
          <p><b>{folder}</b>: {planSummary(plan)}</p>
          <ul className="code-changes">
            {plan.changes.map(c => (
              <li key={c.path} className={c.error ? 'error-row' : c.action}>
                <code><span className="sign">{SIGN[c.error ? 'error' : c.action]}</span> {c.path}</code>
                {c.error && <div className="small error">{c.error}</div>}
                {takesSecret(c.path) && c.action !== 'delete' && (
                  <input type="password" autoComplete="off" className="small" placeholder="Its secret, if new or changed (otherwise the saved one is kept)"
                    value={secrets[c.path] ?? ''} onChange={e => setSecrets(s => ({ ...s, [c.path]: e.target.value }))} />
                )}
              </li>
            ))}
          </ul>
          <div className="row">
            <button onClick={() => run(files, secrets, prune, false)} disabled={busy}>Plan again</button>
            <button className="primary" disabled={busy || !plan.ok || !toApply} onClick={() => run(files, secrets, prune, true)}>
              Apply {toApply} change{toApply === 1 ? '' : 's'}
            </button>
          </div>
        </div>
      )}
      {done && <p className="ok">{done}</p>}
    </div>
  )
}
