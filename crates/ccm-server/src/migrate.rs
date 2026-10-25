//! `ccm-server migrate-from-go`: copy a Go cliproxy database into this one.
//!
//! Users, API keys (bcrypt hashes are compatible), invites, policies, the
//! admin token hash, the global model denylist and usage events keep their
//! ids, so existing `sk-cp-…` keys and the admin token keep working.
//! Upstream accounts (present when the Go version had them) are decrypted
//! with the Go secret key and re-sealed with this one.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use ccm_core::accounts::{Account, AccountStore, Credentials};
use ccm_core::config::Config;
use ccm_core::secret::SecretBox;
use ccm_core::store::Store;
use clap::Args;
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use std::sync::Arc;

#[derive(Args)]
pub struct MigrateArgs {
    /// The Go cliproxy data dir (holds cliproxy.db and secret.key).
    #[arg(long, default_value_os_t = default_go_data_dir())]
    go_data_dir: PathBuf,
    /// The Go secret key (base64); defaults to <go-data-dir>/secret.key.
    #[arg(long, env = "CLIPROXY_SECRET_KEY", hide_env_values = true)]
    go_secret_key: Option<String>,
}

fn default_go_data_dir() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs_home().map(|h| h.join(".local").join("share")))
        .unwrap_or_default()
        .join("cliproxy")
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub users: usize,
    pub keys: usize,
    pub invites: usize,
    pub usage_events: usize,
    pub accounts: usize,
}

pub fn run(cfg: &Config, args: MigrateArgs) -> Result<()> {
    let store = Store::open(&cfg.data_dir)?;
    let secrets = SecretBox::load(&cfg.secret_key, &cfg.data_dir)?;
    let accounts = AccountStore::new(store.clone(), Arc::new(secrets));
    let r = migrate(
        &store,
        &accounts,
        &args.go_data_dir,
        args.go_secret_key.as_deref(),
    )?;
    println!(
        "migrated from {}: {} users, {} api keys, {} invites, {} usage events, {} accounts",
        args.go_data_dir.display(),
        r.users,
        r.keys,
        r.invites,
        r.usage_events,
        r.accounts
    );
    Ok(())
}

pub fn migrate(
    store: &Store,
    accounts: &AccountStore,
    go_dir: &std::path::Path,
    go_key: Option<&str>,
) -> Result<Report> {
    let go_db = go_dir.join("cliproxy.db");
    if !go_db.is_file() {
        bail!("{} not found — pass --go-data-dir", go_db.display());
    }
    {
        let conn = store.conn();
        for table in ["users", "invites", "usage_events"] {
            let n: i64 =
                conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))?;
            if n > 0 {
                bail!(
                    "this CC Manager database already has {table}; migrate into a fresh data_dir"
                );
            }
        }
    }
    let go = Connection::open_with_flags(&go_db, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("open {}", go_db.display()))?;

    // Accounts first (they go through AccountStore, which takes the lock).
    let mut report = Report::default();
    let map = migrate_accounts(&go, accounts, go_dir, go_key, &mut report)?;

    let has_account_col = column_exists(&go, "usage_events", "account_id")?;
    let mut conn = store.conn();
    conn.execute(
        "ATTACH DATABASE ?1 AS go",
        [format!("file:{}?mode=ro", go_db.display())],
    )?;
    let result = (|| -> Result<()> {
        let tx = conn.transaction()?;
        tx.execute_batch(
            "CREATE TEMP TABLE acct_map(old INTEGER PRIMARY KEY, new INTEGER NOT NULL)",
        )?;
        for (old, new) in &map {
            tx.execute("INSERT INTO acct_map(old, new) VALUES(?1, ?2)", [old, new])?;
        }
        report.users = tx.execute("INSERT INTO users(id, label, created_at, enabled) SELECT id, label, created_at, enabled FROM go.users", [])?;
        report.keys = tx.execute(
            "INSERT INTO api_keys(id, user_id, prefix, hash, created_at, last_used_at, revoked_at)
             SELECT id, user_id, prefix, hash, created_at, last_used_at, revoked_at FROM go.api_keys",
            [],
        )?;
        report.invites = tx.execute(
            "INSERT INTO invites(token_hash, label, created_at, expires_at, redeemed_at, redeemed_user_id)
             SELECT token_hash, label, created_at, expires_at, redeemed_at, redeemed_user_id FROM go.invites",
            [],
        )?;
        tx.execute(
            "INSERT INTO policies(user_id, allowed_models, daily_token_limit, rpm_limit)
             SELECT user_id, allowed_models, daily_token_limit, rpm_limit FROM go.policies",
            [],
        )?;
        tx.execute(
            "INSERT OR REPLACE INTO global_settings(key, value)
             SELECT key, value FROM go.global_settings WHERE key IN ('admin_token_hash', 'disabled_models')",
            [],
        )?;
        let account_expr = if has_account_col {
            "(SELECT new FROM acct_map WHERE old = e.account_id)"
        } else {
            "NULL"
        };
        report.usage_events = tx.execute(
            &format!(
                "INSERT INTO usage_events(id, ts, user_id, key_id, account_id, model, input_tokens, output_tokens,
                    cache_read_tokens, cache_creation_tokens, latency_ms, status, error)
                 SELECT e.id, e.ts, e.user_id, e.key_id, {account_expr}, e.model, e.input_tokens, e.output_tokens,
                    e.cache_read_tokens, e.cache_creation_tokens, e.latency_ms, e.status, e.error
                 FROM go.usage_events e"
            ),
            [],
        )?;
        tx.execute_batch("DROP TABLE temp.acct_map")?;
        tx.commit()?;
        Ok(())
    })();
    conn.execute_batch("DETACH DATABASE go")?;
    result?;
    Ok(report)
}

fn table_exists(c: &Connection, table: &str) -> Result<bool> {
    Ok(c.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [table],
        |_| Ok(()),
    )
    .optional()?
    .is_some())
}

fn column_exists(c: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut stmt = c.prepare(&format!("PRAGMA table_info({table})"))?;
    let cols = stmt.query_map([], |r| r.get::<_, String>(1))?;
    for col in cols {
        if col? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Copies Go accounts, returning old id → new id. An account whose source
/// file already backs an account here (e.g. auto-imported on first start)
/// is mapped onto it instead of duplicated.
fn migrate_accounts(
    go: &Connection,
    accounts: &AccountStore,
    go_dir: &std::path::Path,
    go_key: Option<&str>,
    report: &mut Report,
) -> Result<HashMap<i64, i64>> {
    let mut map = HashMap::new();
    if !table_exists(go, "accounts")? {
        return Ok(map);
    }
    let mut stmt = go.prepare(
        "SELECT a.id, a.label, a.email, a.org_uuid, a.plan, a.priority, a.weight, a.enabled, a.status,
                a.status_reason, a.proxy_url, a.source_path, a.sync_file,
                s.access_token_enc, s.refresh_token_enc, s.expires_at, s.scopes
         FROM accounts a JOIN account_secrets s ON s.account_id = a.id ORDER BY a.id",
    )?;
    type Row = (Account, Vec<u8>, Option<Vec<u8>>, i64, String);
    let rows: Vec<Row> = stmt
        .query_map([], |r| {
            Ok((
                Account {
                    id: r.get(0)?,
                    label: r.get(1)?,
                    email: r.get(2)?,
                    org_uuid: r.get(3)?,
                    plan: r.get(4)?,
                    priority: r.get(5)?,
                    weight: r.get(6)?,
                    enabled: r.get(7)?,
                    status: r.get(8)?,
                    status_reason: r.get(9)?,
                    proxy_url: r.get(10)?,
                    source_path: r.get(11)?,
                    sync_file: r.get(12)?,
                    ..Default::default()
                },
                r.get(13)?,
                r.get(14)?,
                r.get(15)?,
                r.get(16)?,
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;
    if rows.is_empty() {
        return Ok(map);
    }
    let key = match go_key {
        Some(k) => k.to_string(),
        None => std::fs::read_to_string(go_dir.join("secret.key")).context(
            "the Go database has accounts but its secret.key is missing — pass --go-secret-key",
        )?,
    };
    let go_box = SecretBox::from_base64(&key).context("Go secret key")?;
    let existing = accounts.list()?;
    for (a, access, refresh, expires_at, scopes) in rows {
        if let Some(e) = existing
            .iter()
            .find(|e| !a.source_path.is_empty() && e.source_path == a.source_path)
        {
            map.insert(a.id, e.id);
            continue;
        }
        let open = |sealed: &[u8], field: &str| -> Result<String> {
            let plain = go_box
                .open(sealed, format!("account:{}:{field}", a.id).as_bytes())
                .with_context(|| format!("decrypt Go account #{} ({field})", a.id))?;
            Ok(String::from_utf8(plain)?)
        };
        let creds = Credentials {
            access_token: open(&access, "access")?,
            refresh_token: match refresh {
                Some(r) if !r.is_empty() => open(&r, "refresh")?,
                _ => String::new(),
            },
            expires_at,
            scopes: scopes.split_whitespace().map(String::from).collect(),
        };
        let old = a.id;
        let new = accounts.create(Account { id: 0, ..a }, &creds)?;
        map.insert(old, new);
        report.accounts += 1;
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ccm_core::auth::{Admin, Keys};
    use rusqlite::params;

    /// The Go cliproxy schema (001_init + 002_accounts).
    const GO_SCHEMA: &str = "
        CREATE TABLE users (id INTEGER PRIMARY KEY, label TEXT NOT NULL, created_at INTEGER NOT NULL, enabled INTEGER NOT NULL DEFAULT 1);
        CREATE TABLE api_keys (id INTEGER PRIMARY KEY, user_id INTEGER NOT NULL, prefix TEXT NOT NULL, hash TEXT NOT NULL,
            created_at INTEGER NOT NULL, last_used_at INTEGER, revoked_at INTEGER);
        CREATE UNIQUE INDEX api_keys_prefix ON api_keys(prefix);
        CREATE TABLE invites (token_hash TEXT PRIMARY KEY, label TEXT NOT NULL, created_at INTEGER NOT NULL,
            expires_at INTEGER NOT NULL, redeemed_at INTEGER, redeemed_user_id INTEGER);
        CREATE TABLE policies (user_id INTEGER PRIMARY KEY, allowed_models TEXT, daily_token_limit INTEGER, rpm_limit INTEGER);
        CREATE TABLE global_settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
        CREATE TABLE usage_events (id INTEGER PRIMARY KEY, ts INTEGER NOT NULL, user_id INTEGER NOT NULL, key_id INTEGER NOT NULL,
            model TEXT NOT NULL, input_tokens INTEGER NOT NULL DEFAULT 0, output_tokens INTEGER NOT NULL DEFAULT 0,
            cache_read_tokens INTEGER NOT NULL DEFAULT 0, cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
            latency_ms INTEGER NOT NULL, status INTEGER NOT NULL, error TEXT);
        CREATE TABLE accounts (id INTEGER PRIMARY KEY, label TEXT NOT NULL, email TEXT NOT NULL DEFAULT '',
            org_uuid TEXT NOT NULL DEFAULT '', plan TEXT NOT NULL DEFAULT '', priority INTEGER NOT NULL DEFAULT 0,
            weight INTEGER NOT NULL DEFAULT 1, enabled INTEGER NOT NULL DEFAULT 1, status TEXT NOT NULL DEFAULT 'active',
            status_reason TEXT NOT NULL DEFAULT '', proxy_url TEXT NOT NULL DEFAULT '', source_path TEXT NOT NULL DEFAULT '',
            sync_file INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
        CREATE TABLE account_secrets (account_id INTEGER PRIMARY KEY, access_token_enc BLOB NOT NULL, refresh_token_enc BLOB,
            expires_at INTEGER NOT NULL DEFAULT 0, scopes TEXT NOT NULL DEFAULT '', updated_at INTEGER NOT NULL);
        ALTER TABLE usage_events ADD COLUMN account_id INTEGER;
    ";

    #[test]
    fn migrates_go_database() {
        let go_dir = tempfile::tempdir().unwrap();
        let go_key =
            SecretBox::from_base64("AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=").unwrap();
        std::fs::write(
            go_dir.path().join("secret.key"),
            "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=\n",
        )
        .unwrap();

        // A Go-issued key: 8-char prefix, bcrypt hash.
        let full = "sk-cp-abcdefghijklmnopqrstuvwxyz234567abcd";
        let admin_hash = bcrypt::hash("go-admin", 4).unwrap();
        {
            let go = Connection::open(go_dir.path().join("cliproxy.db")).unwrap();
            go.execute_batch(GO_SCHEMA).unwrap();
            go.execute("INSERT INTO users VALUES(3, 'alice', 1, 1)", [])
                .unwrap();
            go.execute(
                "INSERT INTO api_keys VALUES(7, 3, ?1, ?2, 1, NULL, NULL)",
                params![&full[..8], bcrypt::hash(full, 4).unwrap()],
            )
            .unwrap();
            go.execute(
                "INSERT INTO invites VALUES('h', 'inv', 1, 2, NULL, NULL)",
                [],
            )
            .unwrap();
            go.execute(
                "INSERT INTO policies VALUES(3, '[\"claude-x\"]', 100, 5)",
                [],
            )
            .unwrap();
            go.execute(
                "INSERT INTO global_settings VALUES('admin_token_hash', ?1)",
                [&admin_hash],
            )
            .unwrap();
            go.execute("INSERT INTO accounts(id, label, created_at, updated_at) VALUES(4, 'go-acct', 1, 1)", []).unwrap();
            go.execute(
                "INSERT INTO account_secrets VALUES(4, ?1, NULL, 5, '', 1)",
                [go_key.seal(b"go-access", b"account:4:access").unwrap()],
            )
            .unwrap();
            go.execute(
                "INSERT INTO usage_events(id, ts, user_id, key_id, model, latency_ms, status, account_id)
                 VALUES(1, 10, 3, 7, 'claude-x', 5, 200, 4)",
                [],
            )
            .unwrap();
        }

        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let accounts =
            AccountStore::new(store.clone(), Arc::new(SecretBox::new(&[2u8; 32]).unwrap()));
        let r = migrate(&store, &accounts, go_dir.path(), None).unwrap();
        assert_eq!(
            r,
            Report {
                users: 1,
                keys: 1,
                invites: 1,
                usage_events: 1,
                accounts: 1
            }
        );

        assert_eq!(Keys::new(store.clone()).lookup(full).unwrap().user_id, 3);
        Admin::new(store.clone()).verify("go-admin").unwrap();
        let acct = &accounts.list().unwrap()[0];
        assert_eq!(
            accounts.load_credentials(acct.id).unwrap().access_token,
            "go-access"
        );
        let mapped: i64 = store
            .conn()
            .query_row(
                "SELECT account_id FROM usage_events WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(mapped, acct.id);

        // Refuses to run twice.
        assert!(migrate(&store, &accounts, go_dir.path(), None).is_err());
    }
}
