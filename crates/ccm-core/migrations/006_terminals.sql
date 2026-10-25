-- Terminal key expiry, a per-terminal IP limit, and the IPs each terminal
-- has used (for the limit and the terminal detail view).
ALTER TABLE api_keys ADD COLUMN expires_at INTEGER;
ALTER TABLE policies ADD COLUMN max_ips INTEGER;
CREATE TABLE user_ips (
  user_id INTEGER NOT NULL,
  ip TEXT NOT NULL,
  first_seen INTEGER NOT NULL,
  last_seen INTEGER NOT NULL,
  requests INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (user_id, ip)
);
CREATE INDEX user_ips_seen ON user_ips(user_id, last_seen);
