/** "Connect your AI agent" (Get started): the settings each agent takes for this app's MCP
 *  endpoint (/api/mcp), to copy and paste. */

export type AgentId = 'claude-code' | 'claude-desktop' | 'codex' | 'cursor'

export const AGENTS: { id: AgentId; name: string }[] = [
  { id: 'claude-code', name: 'Claude Code' },
  { id: 'claude-desktop', name: 'Claude Desktop' },
  { id: 'codex', name: 'Codex' },
  { id: 'cursor', name: 'Cursor' },
]

const TOKEN = 'YOUR_TOKEN'
const json = (v: unknown) => JSON.stringify(v, null, 2)

/** What to paste, and where. `token`: the agent signs in with an API token (always on a server;
 *  in lite only with a PIN). */
export function agentSettings(agent: AgentId, url: string, token: boolean): { where: string; text: string; then?: string } {
  const header = `Authorization: Bearer ${TOKEN}`
  switch (agent) {
    case 'claude-code':
      return {
        where: 'Run this in a terminal:',
        text: `claude mcp add --transport http magnum-bi ${url}${token ? ` --header "${header}"` : ''}`,
      }
    case 'claude-desktop':
      return {
        where: 'Claude Desktop › Settings › Developer › Edit Config (claude_desktop_config.json), then restart Claude Desktop. It needs Node.js: mcp-remote connects it to this computer.',
        text: json({ mcpServers: { 'magnum-bi': { command: 'npx', args: ['-y', 'mcp-remote', url, ...(token ? ['--header', header] : [])] } } }),
      }
    case 'codex':
      return {
        where: 'Add this to ~/.codex/config.toml:',
        text: `[mcp_servers.magnum-bi]\nurl = "${url}"${token ? '\nbearer_token_env_var = "MAGNUM_TOKEN"' : ''}`,
        then: token ? 'Then set MAGNUM_TOKEN to your token where Codex runs.' : undefined,
      }
    case 'cursor':
      return {
        where: 'Add this to ~/.cursor/mcp.json (or Cursor › Settings › MCP › Add):',
        text: json({ mcpServers: { 'magnum-bi': { url, ...(token ? { headers: { Authorization: `Bearer ${TOKEN}` } } : {}) } } }),
      }
  }
}
