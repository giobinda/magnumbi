-- Where each person starts (chat or dashboards), when not where their organisation does.
CREATE TABLE IF NOT EXISTS chat_home (
  org TEXT NOT NULL,
  user_email TEXT NOT NULL,
  home TEXT NOT NULL,
  PRIMARY KEY (org, user_email)
);
