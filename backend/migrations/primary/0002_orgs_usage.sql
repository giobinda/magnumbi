-- Each organisation's dedicated ScramDB instance, and the platform's usage metrics.

-- where an organisation's own ScramDB is: its connection string sealed with MAGNUM_SECRET_KEY
-- (crypto.rs), plus a redacted copy to show in the UI
CREATE TABLE IF NOT EXISTS org_databases (
  org TEXT PRIMARY KEY,
  dsn_sealed TEXT NOT NULL,
  dsn_display TEXT NOT NULL,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_by TEXT
);

-- what happens on the platform (usage.rs): append-only, kept MAGNUM_USAGE_RETENTION_DAYS
CREATE TABLE IF NOT EXISTS usage_events (
  id TEXT PRIMARY KEY,                     -- UUIDv7: time-ordered
  at TIMESTAMPTZ NOT NULL DEFAULT now(),
  kind TEXT NOT NULL,                      -- sign_in | query | dashboard_view | dashboard_saved | ...
  org TEXT NOT NULL DEFAULT '',            -- '' = platform workspace
  user_email TEXT,
  role TEXT,
  source TEXT,                             -- queries: explore | tile | filter_values
  dashboard_id TEXT,
  model TEXT,
  connection TEXT,
  row_count BIGINT,
  duration_ms BIGINT,
  cache_hit BOOLEAN,
  ok BOOLEAN NOT NULL DEFAULT true,
  detail TEXT                              -- small JSON object
);
CREATE INDEX IF NOT EXISTS ix_usage_events_at ON usage_events (at);
CREATE INDEX IF NOT EXISTS ix_usage_events_org_at ON usage_events (org, at);
