/** A tenant's mail server and webhooks (the Delivery page): form values to the API's body. */
import type { MailSecurity } from './api'

export type MailForm = { host: string; port: string; security: MailSecurity; username: string; from: string }

export function defaultPort(security: MailSecurity): number {
  return { starttls: 587, tls: 465, none: 25 }[security]
}

/** An empty password keeps the stored one; without a user name there's no password at all. */
export function mailBody(f: MailForm, password: string): Record<string, unknown> {
  const username = f.username.trim() || null
  const body: Record<string, unknown> = { host: f.host.trim(), port: Number(f.port), security: f.security, username, from: f.from.trim() }
  if (!username) body.password = ''
  else if (password) body.password = password
  return body
}
