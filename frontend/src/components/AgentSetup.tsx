import { useState } from 'react'
import { AGENTS, agentSettings } from '../agentSetup'
import type { AgentId } from '../agentSetup'
import { useAuth } from '../auth'

/** "Connect your AI agent": pick the agent, copy its settings for this app's MCP endpoint. On a
 *  server (and in lite with a PIN) it signs in with an API token, made under API tokens. */
export default function AgentSetup() {
  const { auth } = useAuth()
  const [agent, setAgent] = useState<AgentId>('claude-code')
  const [copied, setCopied] = useState(false)
  const token = !auth?.lite || auth.method === 'pin'
  const s = agentSettings(agent, `${location.origin}/api/mcp`, token)
  const copy = () => navigator.clipboard?.writeText(s.text).then(() => { setCopied(true); setTimeout(() => setCopied(false), 1500) })
  return (
    <div className="agent-setup">
      <div className="agent-pick" role="radiogroup" aria-label="Your agent">
        {AGENTS.map(a => (
          <button key={a.id} type="button" role="radio" aria-checked={a.id === agent}
            className={a.id === agent ? 'primary small' : 'small'} onClick={() => setAgent(a.id)}>{a.name}</button>
        ))}
      </div>
      <p className="small">{s.where}</p>
      <div className="agent-code">
        <pre className="mono">{s.text}</pre>
        <button type="button" className="small" onClick={copy}>{copied ? 'Copied' : 'Copy'}</button>
      </div>
      {s.then && <p className="small">{s.then}</p>}
      <p className="muted small">{token
        ? <>Replace YOUR_TOKEN with an API token (under <b>API tokens</b>): read-only to look around, read and write to let it build dashboards. It acts as you, with your role at most.</>
        : <>No token needed: lite is yours alone, on this computer. The agent then acts as you; set a PIN to require a token.</>}</p>
    </div>
  )
}
