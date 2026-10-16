-- Passkeys (WebAuthn): each credential (public key, counters) as webauthn-rs serialises it,
-- a stable random id per person for the authenticator, and ceremonies in progress.
ALTER TABLE users ADD COLUMN IF NOT EXISTS webauthn_id TEXT;
CREATE TABLE IF NOT EXISTS passkeys (
  id TEXT PRIMARY KEY,
  user_email TEXT NOT NULL,
  name TEXT NOT NULL,
  passkey TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  last_used_at TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS ix_passkeys_user_email ON passkeys (user_email);
CREATE TABLE IF NOT EXISTS webauthn_states (
  ticket_hash TEXT PRIMARY KEY,
  user_email TEXT NOT NULL,
  kind TEXT NOT NULL,
  state TEXT NOT NULL,
  expires_at TIMESTAMPTZ NOT NULL
);
