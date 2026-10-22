import { useEffect, useRef, useState } from 'react'
import { Link } from 'react-router-dom'
import { api } from '../api'
import type { ChartTile, ChatMessage, ChatState, DashboardSummary, Proposal } from '../api'
import { useCan } from '../auth'
import { basedOn, chartTile, suggestions } from '../chat'
import TileView from '../components/TileView'
import { useModels } from '../hooks'
import { addTile } from '../pin'

/** Ask the organisation's data in plain words: answers with charts, and changes to confirm. */
export default function ChatPage() {
  const [state, setState] = useState<ChatState | null>(null)
  const [conversation, setConversation] = useState<string | null>(null)
  const [messages, setMessages] = useState<ChatMessage[]>([])
  const [ideas, setIdeas] = useState<string[]>([])
  const [text, setText] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [pinning, setPinning] = useState<ChartTile | null>(null)
  const { models } = useModels()
  const can = useCan()
  const end = useRef<HTMLDivElement>(null)

  const load = () => api.chat().then(setState, e => setError((e as Error).message))
  useEffect(() => { load() }, [])
  useEffect(() => {
    if (!models.length) return
    api.glossary().then(g => setIdeas(suggestions(models, g.terms)), () => setIdeas(suggestions(models, [])))
  }, [models])
  useEffect(() => { end.current?.scrollIntoView({ behavior: 'smooth', block: 'end' }) }, [messages, busy])

  const open = async (id: string) => {
    setError(null)
    try {
      const c = await api.conversation(id)
      setConversation(id); setMessages(c.messages)
    } catch (e) { setError((e as Error).message) }
  }
  const fresh = () => { setConversation(null); setMessages([]); setError(null) }
  const remove = async (id: string) => {
    try { await api.deleteConversation(id); if (id === conversation) fresh(); load() } catch (e) { setError((e as Error).message) }
  }

  const ask = async (question: string) => {
    const q = question.trim()
    if (!q || busy) return
    setBusy(true); setError(null); setText('')
    setMessages(ms => [...ms, { role: 'user', text: q }])
    try {
      const r = await api.ask(q, conversation)
      if (!conversation) load()
      setConversation(r.conversation)
      setMessages(ms => [...ms, r.reply])
    } catch (e) {
      setError((e as Error).message)
      setMessages(ms => ms.slice(0, -1)); setText(q)
    } finally { setBusy(false) }
  }

  const confirm = async (p: Proposal) => {
    if (!conversation) return
    try {
      const done = await api.applyProposal(conversation, p.id)
      setMessages(ms => ms.map(m => m.role === 'assistant' ? { ...m, proposals: m.proposals.map(x => (x.id === p.id ? done : x)) } : m))
    } catch (e) { setError((e as Error).message) }
  }

  const startHere = async (on: boolean) => {
    try { await api.setHome(on ? 'chat' : 'dashboards'); load() } catch (e) { setError((e as Error).message) }
  }

  if (state && !state.enabled) {
    return (
      <div className="page narrow">
        <h1>Chat</h1>
        <p className="muted">Ask your data questions in plain words. {can('manage_members')
          ? <>Set it up with your organisation's AI provider in <Link to="/chat-settings">Chat settings</Link>.</>
          : 'Your organisation hasn\'t set up the chat yet: ask an Org admin.'}</p>
      </div>
    )
  }

  return (
    <div className="chat">
      <aside className="chat-side">
        <button className="primary" onClick={fresh}>+ New conversation</button>
        {state?.conversations.map(c => (
          <div key={c.id} className={`chat-conv${c.id === conversation ? ' active' : ''}`}>
            <button className="link" onClick={() => open(c.id)} title={c.title}>{c.title}</button>
            <button className="add" title="Delete" onClick={() => remove(c.id)}>×</button>
          </div>
        ))}
        {can('manage_members') && <Link className="small" to="/chat-settings">Chat settings</Link>}
        {state && (
          <label className="check small chat-start">
            <input type="checkbox" checked={state.home === 'chat'} onChange={e => startHere(e.target.checked)} /> Start here when I sign in
          </label>
        )}
      </aside>

      <section className="chat-main">
        <div className="chat-thread">
          {messages.length === 0 && (
            <div className="chat-hello">
              <h1>What would you like to know?</h1>
              <p className="muted">Ask in plain words: the answer comes with the chart it's based on.</p>
              <div className="chat-ideas">
                {ideas.map(i => <button key={i} onClick={() => ask(i)}>{i}</button>)}
              </div>
            </div>
          )}
          {messages.map((m, i) => m.role === 'user'
            ? <div key={i} className="chat-msg user">{m.text}</div>
            : (
              <div key={i} className="chat-msg assistant">
                <p className="chat-text">{m.text}</p>
                {m.charts.map((c, j) => {
                  const tile = chartTile(c.query, i * 10 + j)
                  const model = models.find(x => x.name === c.query.model)
                  const b = basedOn(c.query, models)
                  return (
                    <div key={j} className="chat-chart">
                      <div className="chat-tile"><TileView tile={tile} model={model} /></div>
                      <div className="chat-based small muted">
                        Based on <b>{b.model}</b>{b.status && <span className={`status ${b.status}`}>{b.status}</span>}: {b.fields.join(', ')}
                        <span className="grow" />
                        {can('create') && <button className="link" onClick={() => setPinning(tile)}>Pin to dashboard</button>}
                        {can('explore') && <Link to={`/explore?model=${encodeURIComponent(c.query.model)}`}>Open in Explore</Link>}
                      </div>
                    </div>
                  )
                })}
                {m.proposals.map(p => (
                  <div key={p.id} className={`chat-proposal ${p.status}`}>
                    <div><b>{p.summary}</b></div>
                    {p.status === 'proposed' && <>
                      <p className="small muted">Nothing changes until you confirm.</p>
                      <button className="primary" onClick={() => confirm(p)}>Confirm</button>
                    </>}
                    {p.status === 'applied' && <p className="small">✓ Done.</p>}
                    {p.status === 'failed' && <p className="small error">{p.error}</p>}
                  </div>
                ))}
              </div>
            ))}
          {busy && <div className="chat-msg assistant thinking">Thinking…</div>}
          <div ref={end} />
        </div>
        {error && <div className="error">{error}</div>}
        <form className="chat-ask" onSubmit={e => { e.preventDefault(); ask(text) }}>
          <textarea rows={2} value={text} placeholder="Ask a question about your data…" disabled={busy}
            onChange={e => setText(e.target.value)}
            onKeyDown={e => { if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); ask(text) } }} />
          <button className="primary" disabled={busy || !text.trim()}>Ask</button>
        </form>
      </section>

      {pinning && <PinDialog tile={pinning} onClose={() => setPinning(null)} />}
    </div>
  )
}

function PinDialog({ tile, onClose }: { tile: ChartTile; onClose: () => void }) {
  const [dashboards, setDashboards] = useState<DashboardSummary[]>([])
  const [target, setTarget] = useState('')
  const [title, setTitle] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [done, setDone] = useState(false)
  useEffect(() => { api.dashboards().then(r => { setDashboards(r.dashboards); setTarget(r.dashboards[0]?.id ?? '') }) }, [])
  const pin = async () => {
    try { await addTile(target, { ...tile, title: title.trim() || null }); setDone(true) } catch (e) { setError((e as Error).message) }
  }
  return (
    <div className="modal-bg" onMouseDown={e => { if (e.target === e.currentTarget) onClose() }}>
      <div className="modal">
        <h2>Pin to dashboard</h2>
        {done ? <>
          <p>Pinned. <Link to={`/dashboards/${encodeURIComponent(target)}`}>Open the dashboard</Link></p>
          <div className="row"><button onClick={onClose}>Close</button></div>
        </> : <>
          <select value={target} onChange={e => setTarget(e.target.value)} aria-label="Dashboard">
            {dashboards.map(d => <option key={d.id} value={d.id}>{d.title}</option>)}
          </select>
          <input className="title-input" placeholder="Tile title (optional)" value={title} onChange={e => setTitle(e.target.value)} />
          {error && <div className="error">{error}</div>}
          <div className="row"><button onClick={onClose}>Cancel</button><button className="primary" disabled={!target} onClick={pin}>Pin</button></div>
        </>}
      </div>
    </div>
  )
}
