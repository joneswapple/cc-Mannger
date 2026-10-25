-- Latest known usage of each account's rate-limit windows (see quota.rs).
CREATE TABLE account_quota (
  account_id INTEGER PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
  five_hour_utilization REAL,
  five_hour_reset INTEGER,
  seven_day_utilization REAL,
  seven_day_reset INTEGER,
  status TEXT NOT NULL DEFAULT '',
  updated_at INTEGER NOT NULL
);
