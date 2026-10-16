-- API tokens (api/tokens.rs): only a SHA-256 of the token is stored. A token acts for the person
-- who made it, in one organisation ('' = the platform workspace), with at most `role`.
CREATE TABLE IF NOT EXISTS api_tokens (
  id TEXT PRIMARY KEY,
  token_hash TEXT NOT NULL,
  name TEXT NOT NULL,
  user_email TEXT NOT NULL,
  org TEXT NOT NULL,
  role TEXT NOT NULL,
  write BOOLEAN NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  expires_at TIMESTAMPTZ NOT NULL,
  last_used_at TIMESTAMPTZ,
  revoked_at TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS ix_api_tokens_hash ON api_tokens (token_hash);
CREATE INDEX IF NOT EXISTS ix_api_tokens_org ON api_tokens (org);

-- which token (if any) an action came through: "token:<name>"
ALTER TABLE usage_events ADD COLUMN IF NOT EXISTS via TEXT;
