import { ask } from '../dialogs'
import { useEffect, useState } from 'react'
import { api } from '../api'
import type { ChatSettings } from '../api'

const PRESETS: { label: string; format: ChatSettings['format']; url: string; model: string }[] = [
  { label: 'OpenAI', format: 'openai', url: 'https://api.openai.com/v1', model: 'gpt-4.1' },
  { label: 'Anthropic', format: 'anthropic', url: 'https://api.anthropic.com', model: 'claude-sonnet-5-5' },
  { label: 'Mistral', format: 'openai', url: 'https://api.mistral.ai/v1', model: 'mistral-large-latest' },
  { label: 'Your own server (Ollama, vLLM…)', format: 'openai', url: 'http://localhost:11434/v1', model: 'llama3.1' },
]

const EMPTY: ChatSettings = {
  enabled: false, format: 'openai', url: '', model: '', monthly_questions: 1000, rows_to_model: 200, keep_sensitive: true, home: 'dashboards',
}

/** Org admins: the chat's AI provider (the organisation's own: its key, its costs) and its rules. */
export default function ChatSettingsPage() {
  const [s, setS] = useState<ChatSettings>(EMPTY)
  const [hasKey, setHasKey] = useState(false)
  const [key, setKey] = useState('')
  const [saved, setSaved] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const load = () => api.chatSettings().then(r => { setS(r.settings ?? EMPTY); setHasKey(r.has_key) }, e => setError((e as Error).message))
  useEffect(() => { load() }, [])
  const set = <K extends keyof ChatSettings>(k: K, v: ChatSettings[K]) => { setS(x => ({ ...x, [k]: v })); setSaved(null) }

  const save = async () => {
    setError(null); setSaved(null)
    try { await api.saveChatSettings(s, key); setKey(''); setSaved('Saved.'); load() } catch (e) { setError((e as Error).message) }
  }
  const remove = async () => {
    if (!(await ask.confirm('Remove the chat\'s settings and its key? People\'s conversations stay.', { danger: true, ok: 'Remove' }))) return
    try { await api.deleteChatSettings(); setKey(''); load() } catch (e) { setError((e as Error).message) }
  }

  return (
    <div className="page narrow chat-settings">
      <h1>Chat settings</h1>
      <p className="muted">The chat answers people's questions with your data, using your organisation's own AI provider: you choose it,
        and its key and costs are yours. Magnum BI runs the queries, as the person asking and with their data restrictions, and sends the
        provider only what's needed to answer (see below). Changes the chat suggests are made only when the person confirms them.</p>
      {error && <div className="error">{error}</div>}

      <label className="check"><input type="checkbox" checked={s.enabled} onChange={e => set('enabled', e.target.checked)} /> The chat is on</label>

      <h2 className="section">Provider</h2>
      <div className="row wrap presets">
        {PRESETS.map(p => <button key={p.label} onClick={() => setS(x => ({ ...x, format: p.format, url: p.url, model: x.model || p.model }))}>{p.label}</button>)}
      </div>
      <label>Format
        <select value={s.format} onChange={e => set('format', e.target.value as ChatSettings['format'])}>
          <option value="openai">OpenAI-compatible (OpenAI, Azure, Mistral, OpenRouter, Ollama…)</option>
          <option value="anthropic">Anthropic</option>
        </select>
      </label>
      <label>API address <input value={s.url} placeholder="https://api.openai.com/v1" onChange={e => set('url', e.target.value)} /></label>
      <label>Model <input value={s.model} placeholder="e.g. gpt-4.1" onChange={e => set('model', e.target.value)} /></label>
      <label>API key <span className="muted small">{hasKey ? '(saved: leave empty to keep it)' : '(kept sealed; never shown again)'}</span>
        <input type="password" autoComplete="off" value={key} placeholder={hasKey ? '••••••••' : 'sk-…'} onChange={e => setKey(e.target.value)} />
      </label>

      <h2 className="section">What leaves your servers</h2>
      <label>Rows of each result the provider sees <span className="muted small">(the chart always shows them all)</span>
        <input type="number" min={1} max={5000} value={s.rows_to_model} onChange={e => set('rows_to_model', Math.max(1, Number(e.target.value) || 1))} />
      </label>
      <label className="check"><input type="checkbox" checked={s.keep_sensitive} onChange={e => set('keep_sensitive', e.target.checked)} />
        Never send the values of fields tagged pii, personal, confidential or restricted (counting them is fine)</label>

      <h2 className="section">Use</h2>
      <label>Questions a month, for the whole organisation <span className="muted small">(0: no limit)</span>
        <input type="number" min={0} value={s.monthly_questions} onChange={e => set('monthly_questions', Math.max(0, Number(e.target.value) || 0))} />
      </label>
      <label>People start on
        <select value={s.home} onChange={e => set('home', e.target.value as ChatSettings['home'])}>
          <option value="dashboards">Dashboards</option><option value="chat">The chat</option>
        </select>
      </label>
      <p className="muted small">Everyone can choose differently for themselves, on the chat page.</p>

      <div className="row">
        {hasKey && <button className="danger" onClick={remove}>Remove</button>}
        {saved && <span className="muted small">{saved}</span>}
        <button className="primary" onClick={save}>Save</button>
      </div>
    </div>
  )
}
