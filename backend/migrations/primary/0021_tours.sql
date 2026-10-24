-- Who has seen (or declined) the first-time tour, per organisation: it isn't offered again.
CREATE TABLE IF NOT EXISTS tours (
  org TEXT NOT NULL,
  user_email TEXT NOT NULL,
  PRIMARY KEY (org, user_email)
);
