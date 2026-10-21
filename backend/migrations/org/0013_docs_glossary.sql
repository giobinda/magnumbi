-- What an organisation says about its models (governance::ModelDocs, as JSON), and its glossary
-- of business terms (governance::Term; `name_key`: the name in lower case, unique).
CREATE TABLE IF NOT EXISTS model_docs (
  org TEXT NOT NULL,
  model TEXT NOT NULL,
  definition TEXT NOT NULL,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_by TEXT,
  PRIMARY KEY (org, model)
);
CREATE TABLE IF NOT EXISTS glossary (
  org TEXT NOT NULL,
  name_key TEXT NOT NULL,
  definition TEXT NOT NULL,
  owner TEXT,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_by TEXT,
  PRIMARY KEY (org, name_key)
);
