-- When the account last served a successful request (unix seconds, 0 = never).
ALTER TABLE accounts ADD COLUMN last_used_at INTEGER NOT NULL DEFAULT 0;
