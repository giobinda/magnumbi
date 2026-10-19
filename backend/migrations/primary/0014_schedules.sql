-- Scheduled reports (api/schedules.rs): a dashboard sent on a schedule, as its maker, through the
-- tenant's own mail server and webhooks. A signed-link report keeps the hash of its password.
CREATE TABLE IF NOT EXISTS schedules (
  org TEXT NOT NULL,
  id TEXT NOT NULL,
  dashboard_id TEXT NOT NULL,
  created_by TEXT NOT NULL,
  config TEXT NOT NULL,
  password_hash TEXT,
  active BOOLEAN NOT NULL DEFAULT true,
  next_run_at TIMESTAMPTZ NOT NULL,
  last_run_at TIMESTAMPTZ,
  last_status TEXT,
  failures BIGINT NOT NULL DEFAULT 0,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (org, id)
);
