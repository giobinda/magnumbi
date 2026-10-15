-- The organisation's own blends (blend.rs), defined in the UI: the definition as JSON.
CREATE TABLE IF NOT EXISTS blends (
  org TEXT NOT NULL,
  name TEXT NOT NULL,
  definition TEXT NOT NULL,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_by TEXT,
  PRIMARY KEY (org, name)
);
