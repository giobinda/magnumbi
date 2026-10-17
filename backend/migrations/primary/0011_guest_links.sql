-- Guest links (api/guest.rs): a dashboard for anyone with the link and its password, until it
-- expires (at most 14 days), with a role up to its maker's (Viewer by default), narrowed by the
-- link's own filters.
CREATE TABLE IF NOT EXISTS guest_links (
  org TEXT NOT NULL,
  token_hash TEXT NOT NULL,
  token_sealed TEXT NOT NULL,
  dashboard_id TEXT NOT NULL,
  password_hash TEXT NOT NULL,
  role TEXT NOT NULL,
  filters TEXT NOT NULL,
  expires_at TIMESTAMPTZ NOT NULL,
  created_by TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (org, token_hash)
);
