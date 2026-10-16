-- Authenticator codes (auth::totp): the secret sealed with MAGNUM_SECRET_KEY, a pending one until
-- its first code confirms it, the last step used (a code is good once), and recovery codes as
-- SHA-256 (a JSON list).
ALTER TABLE users ADD COLUMN IF NOT EXISTS totp_secret_sealed TEXT;
ALTER TABLE users ADD COLUMN IF NOT EXISTS totp_pending_sealed TEXT;
ALTER TABLE users ADD COLUMN IF NOT EXISTS totp_last_step BIGINT;
ALTER TABLE users ADD COLUMN IF NOT EXISTS recovery_codes TEXT;

-- sign-ins between the password and the code: only a hash of the ticket is stored
CREATE TABLE IF NOT EXISTS login_challenges (
  ticket_hash TEXT PRIMARY KEY,
  user_email TEXT NOT NULL,
  attempts INTEGER NOT NULL DEFAULT 0,
  expires_at TIMESTAMPTZ NOT NULL
);
