-- An organisation's own ScramDB: its dashboards and its query cache. Rows carry the organisation
-- id, so a development instance can serve several organisations (or be the primary itself).

CREATE TABLE IF NOT EXISTS dashboards (
  org TEXT NOT NULL DEFAULT '',
  id TEXT NOT NULL,
  yaml TEXT NOT NULL,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_by TEXT,
  PRIMARY KEY (org, id)
);

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

-- query results, materialised as typed tables (cache.rs / materialize.rs); the same tables are
-- what a federated query will join across sources
CREATE SCHEMA IF NOT EXISTS magnum_cache;
CREATE TABLE IF NOT EXISTS magnum_cache.entries (
  key TEXT PRIMARY KEY,                    -- SHA-256 of organisation + connection + SQL
  org TEXT NOT NULL,
  model TEXT NOT NULL,
  connection TEXT NOT NULL,
  table_name TEXT NOT NULL,                -- magnum_cache.<table_name> holds the rows
  columns TEXT NOT NULL,                   -- JSON: [{"name", "type"}]
  row_count BIGINT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  expires_at TIMESTAMPTZ NOT NULL,
  last_hit_at TIMESTAMPTZ,
  hits BIGINT NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS ix_cache_entries_expires_at ON magnum_cache.entries (expires_at);
