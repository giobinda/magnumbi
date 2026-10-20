-- Alerts are schedules that check a number (api/schedules.rs): whether it was last seen over
-- the line, so people are told once when it crosses, and again only after it came back.
ALTER TABLE schedules ADD COLUMN IF NOT EXISTS firing BOOLEAN NOT NULL DEFAULT false;
