// Setup as code: the organisation's setup as YAML files, and plans of applying files (api/code.rs).

export interface PlanChange { path: string; action: 'create' | 'update' | 'delete' | 'error'; error?: string }
export interface Plan { ok: boolean; changes: PlanChange[]; unchanged: number }

/** A chosen folder's YAML files by their path inside it (the folder's own name dropped; hidden folders such as .git skipped). */
export function folderFiles(list: { path: string; text: string }[]): Record<string, string> {
  const out: Record<string, string> = {}
  for (const f of list) {
    const parts = f.path.split('/').slice(1)
    if (!parts.length || parts.some(p => p.startsWith('.')) || !f.path.endsWith('.yaml')) continue
    out[parts.join('/')] = f.text
  }
  return out
}

/** Files whose secret (a password, a key, an address) travels apart, never in the file. */
export function takesSecret(path: string): boolean {
  return ['mail.yaml', 'chat.yaml', 'sign_in.yaml'].includes(path) || /^(connections|webhooks|reports)\//.test(path)
}

/** A plan in words: what it creates, changes and deletes, and its problems. */
export function planSummary(plan: Plan): string {
  if (!plan.changes.length) return `Nothing to change (${plan.unchanged} files)`
  const count = (a: PlanChange['action']) => plan.changes.filter(c => c.action === a).length
  const problems = plan.changes.filter(c => c.error).length
  const parts = [[count('create'), 'to create'], [count('update'), 'to change'], [count('delete'), 'to delete']]
    .filter(([n]) => n).map(([n, w]) => `${n} ${w}`)
  const tail = problems ? `${problems} problem${problems > 1 ? 's' : ''} to fix first` : `${plan.unchanged} unchanged`
  return `${parts.join(', ')}; ${tail}`
}
