import { describe, expect, it } from 'vitest'
import { AGENTS, agentSettings } from './agentSetup'

const URL = 'http://localhost:43209/api/mcp'

describe('connecting an AI agent', () => {
  it('offers the usual agents', () => {
    expect(AGENTS.map(a => a.id)).toEqual(['claude-code', 'claude-desktop', 'codex', 'cursor'])
  })

  it('gives each one its settings with this address', () => {
    for (const a of AGENTS) {
      const s = agentSettings(a.id, URL, false)
      expect(s.text, a.id).toContain(URL)
      expect(s.text, a.id).not.toContain('TOKEN')
      expect(s.where, a.id).not.toBe('')
    }
    expect(agentSettings('claude-code', URL, false).text).toBe(`claude mcp add --transport http magnum-bi ${URL}`)
    // Claude Desktop reaches a local server through a bridge it starts itself
    expect(JSON.parse(agentSettings('claude-desktop', URL, false).text))
      .toEqual({ mcpServers: { 'magnum-bi': { command: 'npx', args: ['-y', 'mcp-remote', URL] } } })
    expect(agentSettings('codex', URL, false).text).toBe(`[mcp_servers.magnum-bi]\nurl = "${URL}"`)
    expect(JSON.parse(agentSettings('cursor', URL, false).text)).toEqual({ mcpServers: { 'magnum-bi': { url: URL } } })
  })

  it('adds the API token where one is needed (a server, or lite with a PIN)', () => {
    expect(agentSettings('claude-code', URL, true).text).toContain('--header "Authorization: Bearer YOUR_TOKEN"')
    expect(JSON.parse(agentSettings('claude-desktop', URL, true).text).mcpServers['magnum-bi'].args)
      .toEqual(['-y', 'mcp-remote', URL, '--header', 'Authorization: Bearer YOUR_TOKEN'])
    expect(agentSettings('codex', URL, true).text).toContain('bearer_token_env_var = "MAGNUM_TOKEN"')
    expect(JSON.parse(agentSettings('cursor', URL, true).text).mcpServers['magnum-bi'].headers)
      .toEqual({ Authorization: 'Bearer YOUR_TOKEN' })
  })
})
