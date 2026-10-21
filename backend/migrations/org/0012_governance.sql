-- Data governance (governance.rs): each model's owner, stewards and status in this organisation,
-- as JSON. No row: nobody owns the model yet.
CREATE TABLE IF NOT EXISTS governance (
  org TEXT NOT NULL,
  model TEXT NOT NULL,
  definition TEXT NOT NULL,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_by TEXT,
  PRIMARY KEY (org, model)
);
