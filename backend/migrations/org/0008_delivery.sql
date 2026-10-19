-- Where a tenant (an organisation, or the platform workspace) sends reports and alerts from
-- (delivery.rs): its own mail server and webhooks. Secrets are sealed for the tenant; nothing is
-- shared between tenants.
CREATE TABLE IF NOT EXISTS mail_servers (
  org TEXT NOT NULL PRIMARY KEY,
  config TEXT NOT NULL,
  password_sealed TEXT,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_by TEXT
);

-- the address is the secret (whoever has it can post); only its host is kept in clear
CREATE TABLE IF NOT EXISTS webhooks (
  org TEXT NOT NULL,
  name TEXT NOT NULL,
  kind TEXT NOT NULL,
  host TEXT NOT NULL,
  url_sealed TEXT NOT NULL,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_by TEXT,
  PRIMARY KEY (org, name)
);
