-- The organisation's own themes and default themes (themes.rs OrgThemes, as JSON).
CREATE TABLE IF NOT EXISTS theme_settings (
  org TEXT PRIMARY KEY,
  definition TEXT NOT NULL,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_by TEXT
);
