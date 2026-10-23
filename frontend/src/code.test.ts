import { describe, expect, it } from 'vitest'
import { folderFiles, planSummary, takesSecret } from './code'

describe('setup as code', () => {
  it('reads a chosen folder as the files inside it', () => {
    expect(folderFiles([
      { path: 'acme-setup/models/sales.yaml', text: 'name: sales\n' },
      { path: 'acme-setup/mail.yaml', text: 'host: m\n' },
      { path: 'acme-setup/.git/config.yaml', text: 'x' },
      { path: 'acme-setup/README.md', text: 'notes' },
    ])).toEqual({ 'models/sales.yaml': 'name: sales\n', 'mail.yaml': 'host: m\n' })
  })

  it('knows which files may come with a secret', () => {
    for (const p of ['connections/warehouse.yaml', 'mail.yaml', 'webhooks/team.yaml', 'chat.yaml', 'sign_in.yaml', 'reports/d/monday.yaml']) {
      expect(takesSecret(p)).toBe(true)
    }
    for (const p of ['models/sales.yaml', 'members.yaml', 'dashboards/d.yaml']) expect(takesSecret(p)).toBe(false)
  })

  it('sums a plan up in words', () => {
    const changes = [
      { path: 'a', action: 'create' as const }, { path: 'b', action: 'create' as const },
      { path: 'c', action: 'update' as const }, { path: 'd', action: 'delete' as const },
    ]
    expect(planSummary({ ok: true, changes, unchanged: 4 })).toBe('2 to create, 1 to change, 1 to delete; 4 unchanged')
    expect(planSummary({ ok: true, changes: [], unchanged: 3 })).toBe('Nothing to change (3 files)')
    expect(planSummary({ ok: false, changes: [{ path: 'a', action: 'create', error: 'x' }], unchanged: 0 }))
      .toBe('1 to create; 1 problem to fix first')
  })
})
