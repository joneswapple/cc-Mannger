-- Scheduler state. cooldown_until (unix seconds) parks an account after a
-- 429/403 until the upstream says it resets; it survives restarts.
ALTER TABLE accounts ADD COLUMN cooldown_until INTEGER NOT NULL DEFAULT 0;
-- Accounts can be grouped; a user whose policy names a group only uses
-- accounts in it. Empty means ungrouped / any.
ALTER TABLE accounts ADD COLUMN account_group TEXT NOT NULL DEFAULT '';
ALTER TABLE policies ADD COLUMN account_group TEXT;
