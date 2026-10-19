import { describe, expect, it } from 'vitest'
import { defaultPort, mailBody } from './delivery'

describe('delivery settings', () => {
  it('suggest the usual port for each kind of security', () => {
    expect([defaultPort('starttls'), defaultPort('tls'), defaultPort('none')]).toEqual([587, 465, 25])
  })

  it('keep the stored password unless a new one is typed', () => {
    const form = { host: ' smtp.example.com ', port: '587', security: 'starttls' as const, username: 'bi', from: 'bi@example.com' }
    expect(mailBody(form, '')).toEqual({ host: 'smtp.example.com', port: 587, security: 'starttls', username: 'bi', from: 'bi@example.com' })
    expect(mailBody(form, 'new-pw')).toEqual({ host: 'smtp.example.com', port: 587, security: 'starttls', username: 'bi', from: 'bi@example.com', password: 'new-pw' })
    // no user name: no password either
    expect(mailBody({ ...form, username: ' ', security: 'none' }, 'x')).toEqual({ host: 'smtp.example.com', port: 587, security: 'none', username: null, from: 'bi@example.com', password: '' })
  })
})
