CREATE TABLE IF NOT EXISTS users (
  id INTEGER PRIMARY KEY,
  label TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE IF NOT EXISTS api_keys (
  id INTEGER PRIMARY KEY,
  user_id INTEGER NOT NULL REFERENCES users(id),
  prefix TEXT NOT NULL,
  hash TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  last_used_at INTEGER,
  revoked_at INTEGER
);
CREATE UNIQUE INDEX IF NOT EXISTS api_keys_prefix ON api_keys(prefix);
CREATE INDEX IF NOT EXISTS api_keys_user ON api_keys(user_id);

CREATE TABLE IF NOT EXISTS invites (
  token_hash TEXT PRIMARY KEY,
  label TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL,
  redeemed_at INTEGER,
  redeemed_user_id INTEGER REFERENCES users(id)
);

CREATE TABLE IF NOT EXISTS policies (
  user_id INTEGER PRIMARY KEY REFERENCES users(id),
  allowed_models TEXT,
  daily_token_limit INTEGER,
  rpm_limit INTEGER
);

CREATE TABLE IF NOT EXISTS global_settings (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS usage_events (
  id INTEGER PRIMARY KEY,
  ts INTEGER NOT NULL,
  user_id INTEGER NOT NULL,
  key_id INTEGER NOT NULL,
  model TEXT NOT NULL,
  input_tokens INTEGER NOT NULL DEFAULT 0,
  output_tokens INTEGER NOT NULL DEFAULT 0,
  cache_read_tokens INTEGER NOT NULL DEFAULT 0,
  cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
  latency_ms INTEGER NOT NULL,
  status INTEGER NOT NULL,
  error TEXT
);
CREATE INDEX IF NOT EXISTS usage_events_ts ON usage_events(ts);
CREATE INDEX IF NOT EXISTS usage_events_user_ts ON usage_events(user_id, ts);
