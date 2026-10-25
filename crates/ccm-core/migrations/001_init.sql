-- Terminal users and the sk-cp-… keys cliproxy issues them (ported from the
-- Go cliproxy schema so its database can be migrated in R1).
CREATE TABLE users (
  id INTEGER PRIMARY KEY,
  label TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE api_keys (
  id INTEGER PRIMARY KEY,
  user_id INTEGER NOT NULL REFERENCES users(id),
  prefix TEXT NOT NULL,
  hash TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  last_used_at INTEGER,
  revoked_at INTEGER
);
CREATE UNIQUE INDEX api_keys_prefix ON api_keys(prefix);
CREATE INDEX api_keys_user ON api_keys(user_id);

CREATE TABLE invites (
  token_hash TEXT PRIMARY KEY,
  label TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL,
  redeemed_at INTEGER,
  redeemed_user_id INTEGER REFERENCES users(id)
);

CREATE TABLE policies (
  user_id INTEGER PRIMARY KEY REFERENCES users(id),
  allowed_models TEXT,
  daily_token_limit INTEGER,
  rpm_limit INTEGER
);

CREATE TABLE global_settings (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

-- Upstream Claude Pro/Max OAuth accounts. Only OAuth accounts exist; there
-- is no API-key account type.
CREATE TABLE accounts (
  id INTEGER PRIMARY KEY,
  label TEXT NOT NULL,
  email TEXT NOT NULL DEFAULT '',
  org_uuid TEXT NOT NULL DEFAULT '',
  plan TEXT NOT NULL DEFAULT '',
  priority INTEGER NOT NULL DEFAULT 0,
  weight INTEGER NOT NULL DEFAULT 1,
  enabled INTEGER NOT NULL DEFAULT 1,
  status TEXT NOT NULL DEFAULT 'active',
  status_reason TEXT NOT NULL DEFAULT '',
  proxy_url TEXT NOT NULL DEFAULT '',
  -- source_path is the Claude Code credentials file the account was
  -- imported from. With sync_file = 1 that file stays authoritative: it is
  -- re-read before refreshes and rewritten after them, so the host's own
  -- `claude` CLI and CC Manager can keep sharing one refresh token.
  source_path TEXT NOT NULL DEFAULT '',
  sync_file INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

-- Tokens are AES-256-GCM sealed by secret.rs; never stored in plaintext.
CREATE TABLE account_secrets (
  account_id INTEGER PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
  access_token_enc BLOB NOT NULL,
  refresh_token_enc BLOB,
  expires_at INTEGER NOT NULL DEFAULT 0,
  scopes TEXT NOT NULL DEFAULT '',
  updated_at INTEGER NOT NULL
);

CREATE TABLE usage_events (
  id INTEGER PRIMARY KEY,
  ts INTEGER NOT NULL,
  user_id INTEGER NOT NULL,
  key_id INTEGER NOT NULL,
  account_id INTEGER,
  model TEXT NOT NULL,
  input_tokens INTEGER NOT NULL DEFAULT 0,
  output_tokens INTEGER NOT NULL DEFAULT 0,
  cache_read_tokens INTEGER NOT NULL DEFAULT 0,
  cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
  latency_ms INTEGER NOT NULL,
  status INTEGER NOT NULL,
  error TEXT
);
CREATE INDEX usage_events_ts ON usage_events(ts);
CREATE INDEX usage_events_user_ts ON usage_events(user_id, ts);
CREATE INDEX usage_events_account_ts ON usage_events(account_id, ts);
