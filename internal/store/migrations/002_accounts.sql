CREATE TABLE IF NOT EXISTS accounts (
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
  -- source_path is the Claude Code credentials file this account was
  -- imported from. When sync_file is 1 the file stays authoritative: it is
  -- re-read before refreshes and rewritten after them, so the host's own
  -- `claude` CLI and cliproxy can keep sharing one refresh token.
  source_path TEXT NOT NULL DEFAULT '',
  sync_file INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

-- Tokens are AES-GCM sealed by internal/secret; never stored in plaintext.
CREATE TABLE IF NOT EXISTS account_secrets (
  account_id INTEGER PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
  access_token_enc BLOB NOT NULL,
  refresh_token_enc BLOB,
  expires_at INTEGER NOT NULL DEFAULT 0,
  scopes TEXT NOT NULL DEFAULT '',
  updated_at INTEGER NOT NULL
);

ALTER TABLE usage_events ADD COLUMN account_id INTEGER;
CREATE INDEX IF NOT EXISTS usage_events_account_ts ON usage_events(account_id, ts);
