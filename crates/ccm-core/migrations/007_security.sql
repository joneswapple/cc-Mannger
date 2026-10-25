-- IP allow / deny rules, the IP access log for /v1/* and /register, and the
-- shared proxy list.
CREATE TABLE ip_rules (
  id INTEGER PRIMARY KEY,
  kind TEXT NOT NULL CHECK (kind IN ('allow', 'deny')),
  cidr TEXT NOT NULL,
  note TEXT NOT NULL DEFAULT '',
  expires_at INTEGER,
  created_at INTEGER NOT NULL,
  hits INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE ip_access (
  id INTEGER PRIMARY KEY,
  ts INTEGER NOT NULL,
  ip TEXT NOT NULL,
  method TEXT NOT NULL,
  path TEXT NOT NULL,
  status INTEGER NOT NULL,
  blocked INTEGER NOT NULL DEFAULT 0,
  rule_id INTEGER
);
CREATE INDEX ip_access_ts ON ip_access(ts);
CREATE INDEX ip_access_ip ON ip_access(ip, ts);
CREATE TABLE proxies (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL,
  url TEXT NOT NULL UNIQUE,
  created_at INTEGER NOT NULL
);
