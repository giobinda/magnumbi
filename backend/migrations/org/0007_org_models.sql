-- The organisation's own models (org_models.rs), made in the app: the definition as JSON. Unlike
-- the project's YAML models, they carry no SQL (columns and aggregations only).
CREATE TABLE IF NOT EXISTS org_models (
  org TEXT NOT NULL,
  name TEXT NOT NULL,
  definition TEXT NOT NULL,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_by TEXT,
  PRIMARY KEY (org, name)
);
