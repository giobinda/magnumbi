-- The platform workspace's own data connections (same table as in organisation databases).
-- with MAGNUM_SECRET_KEY for this organisation and connection.
CREATE TABLE IF NOT EXISTS connections (
  org TEXT NOT NULL,
  name TEXT NOT NULL,
  config TEXT NOT NULL,
  secret_sealed TEXT,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_by TEXT,
  PRIMARY KEY (org, name)
);
