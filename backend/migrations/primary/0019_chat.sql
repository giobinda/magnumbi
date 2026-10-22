-- The in-app chat (api/chat.rs): the tenant's provider settings (chat::ChatSettings as JSON; the
-- API key sealed apart, write-only), each person's conversations, and questions asked per month.
CREATE TABLE IF NOT EXISTS chat_settings (
  org TEXT NOT NULL PRIMARY KEY,
  config TEXT NOT NULL,
  key_sealed TEXT,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_by TEXT
);
CREATE TABLE IF NOT EXISTS chat_conversations (
  org TEXT NOT NULL,
  id TEXT NOT NULL,
  user_email TEXT NOT NULL,
  title TEXT NOT NULL,
  -- the conversation as the provider sees it, and as the person does (with charts and proposals)
  turns TEXT NOT NULL,
  shown TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (org, id)
);
CREATE TABLE IF NOT EXISTS chat_usage (
  org TEXT NOT NULL,
  month TEXT NOT NULL,
  questions BIGINT NOT NULL,
  PRIMARY KEY (org, month)
);
