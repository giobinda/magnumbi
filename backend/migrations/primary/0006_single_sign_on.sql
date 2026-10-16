-- Organisations' own OpenID Connect providers: which one a sign-in in progress is for, and the
-- role a session got from the provider's groups ("<org>:<role>").
ALTER TABLE oauth_states ADD COLUMN IF NOT EXISTS provider TEXT;
ALTER TABLE sessions ADD COLUMN IF NOT EXISTS granted_role TEXT;
