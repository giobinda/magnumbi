-- Public links to a dashboard (share.rs): anyone with the link can see it, without signing in.
-- The token is kept sealed (to show the link again) and hashed (to find it).
CREATE TABLE IF NOT EXISTS share_links (
  org TEXT NOT NULL,
  token_hash TEXT NOT NULL,
  token_sealed TEXT NOT NULL,
  dashboard_id TEXT NOT NULL,
  cache_seconds BIGINT NOT NULL,
  created_by TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (org, token_hash)
);

-- What a public link's visitors are shown, per tile, until it expires: the data warehouse is
-- asked at most once per tile per cache period, however many people open the link.
CREATE TABLE IF NOT EXISTS shared_results (
  org TEXT NOT NULL,
  token_hash TEXT NOT NULL,
  tile_id TEXT NOT NULL,
  dashboard_version TEXT NOT NULL,
  result TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  expires_at TIMESTAMPTZ NOT NULL,
  PRIMARY KEY (org, token_hash, tile_id)
);
