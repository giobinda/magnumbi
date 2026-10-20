-- Shared formulas (api/formulas.rs): calculations Org admins add to a model, for everyone in the
-- organisation. The definition is the formula as JSON (name, label, expr, format, description).
CREATE TABLE IF NOT EXISTS formulas (
  org TEXT NOT NULL,
  model TEXT NOT NULL,
  name TEXT NOT NULL,
  definition TEXT NOT NULL,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_by TEXT,
  PRIMARY KEY (org, model, name)
);
