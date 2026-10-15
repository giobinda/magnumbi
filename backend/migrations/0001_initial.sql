-- Magnum BI's own state in ScramDB. ScramDB DDL is not transactional, so every statement is
-- idempotent: a migration interrupted halfway is simply run again.

-- people who can sign in (password and/or Google); identified by lowercased email
CREATE TABLE IF NOT EXISTS users (
  email TEXT PRIMARY KEY,
  name TEXT NOT NULL DEFAULT '',
  password_hash TEXT,                      -- argon2id PHC string; NULL: no password sign-in
  google_sub TEXT,                         -- Google account id, bound on first Google sign-in
  owner_org TEXT,                          -- organisation whose admin set the password (see auth::password)
  must_change_password BOOLEAN NOT NULL DEFAULT false,
  disabled BOOLEAN NOT NULL DEFAULT false,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  last_login_at TIMESTAMPTZ
);

-- sign-in sessions; only a SHA-256 of the cookie value is stored
CREATE TABLE IF NOT EXISTS sessions (
  id_hash TEXT PRIMARY KEY,
  user_email TEXT NOT NULL,
  method TEXT NOT NULL,                    -- password | google
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  expires_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX IF NOT EXISTS ix_sessions_user_email ON sessions (user_email);
CREATE INDEX IF NOT EXISTS ix_sessions_expires_at ON sessions (expires_at);

-- Google sign-ins in progress (PKCE verifier, nonce), keyed by a hash of the OAuth state
CREATE TABLE IF NOT EXISTS oauth_states (
  state_hash TEXT PRIMARY KEY,
  verifier TEXT NOT NULL,
  nonce TEXT NOT NULL,
  redirect_uri TEXT NOT NULL,
  next_url TEXT NOT NULL,
  expires_at TIMESTAMPTZ NOT NULL
);

-- dashboards per organisation ('' = the platform workspace: system owners' templates)
CREATE TABLE IF NOT EXISTS dashboards (
  org TEXT NOT NULL DEFAULT '',
  id TEXT NOT NULL,
  yaml TEXT NOT NULL,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_by TEXT,
  PRIMARY KEY (org, id)
);

-- every save and delete, for history
CREATE TABLE IF NOT EXISTS dashboard_versions (
  id TEXT PRIMARY KEY,
  org TEXT NOT NULL DEFAULT '',
  dashboard_id TEXT NOT NULL,
  yaml TEXT,
  deleted BOOLEAN NOT NULL DEFAULT false,
  saved_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  saved_by TEXT
);
CREATE INDEX IF NOT EXISTS ix_dashboard_versions_dashboard ON dashboard_versions (org, dashboard_id);

-- small JSON documents: "platform", "members:<org>"
CREATE TABLE IF NOT EXISTS settings (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_by TEXT
);
