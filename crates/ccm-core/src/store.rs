//! SQLite connection and embedded migrations.

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension};

/// Migrations in apply order. Each runs once, inside a transaction, and is
/// recorded in `schema_migrations`, so later ones may use non-idempotent
/// statements such as `ALTER TABLE … ADD COLUMN`.
const MIGRATIONS: &[(&str, &str)] = &[
    ("001_init", include_str!("../migrations/001_init.sql")),
    (
        "002_key_prefix_index",
        include_str!("../migrations/002_key_prefix_index.sql"),
    ),
    ("003_pool", include_str!("../migrations/003_pool.sql")),
    ("004_quota", include_str!("../migrations/004_quota.sql")),
];

pub const DB_FILE: &str = "cc-manager.db";

/// A shared handle to the SQLite database. Queries are short, so a single
/// connection behind a mutex is enough; clone the handle freely.
#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
}

impl Store {
    /// Opens (creating if needed) `data_dir/cc-manager.db` and applies
    /// pending migrations.
    pub fn open(data_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(data_dir)
            .with_context(|| format!("create data dir {}", data_dir.display()))?;
        restrict_dir(data_dir);
        let conn = Connection::open(data_dir.join(DB_FILE)).context("open database")?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        Self::init(conn)
    }

    /// An in-memory database, for tests.
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let store = Store {
            conn: Arc::new(Mutex::new(conn)),
        };
        store.migrate()?;
        Ok(store)
    }

    /// Locks the connection. Keep the guard for one short query batch; never
    /// hold it across an `.await`.
    pub fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn migrate(&self) -> Result<()> {
        let mut conn = self.conn();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                name TEXT PRIMARY KEY,
                applied_at INTEGER NOT NULL
            )",
        )?;
        for (name, sql) in MIGRATIONS {
            let applied: Option<i64> = conn
                .query_row(
                    "SELECT applied_at FROM schema_migrations WHERE name = ?1",
                    [name],
                    |r| r.get(0),
                )
                .optional()?;
            if applied.is_some() {
                continue;
            }
            let tx = conn.transaction()?;
            tx.execute_batch(sql)
                .with_context(|| format!("migrate {name}"))?;
            tx.execute(
                "INSERT INTO schema_migrations(name, applied_at) VALUES(?1, ?2)",
                rusqlite::params![name, crate::now_unix()],
            )?;
            tx.commit()?;
        }
        Ok(())
    }

    pub fn get_setting(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT value FROM global_settings WHERE key = ?1",
                [key],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn().execute(
            "INSERT INTO global_settings(key, value) VALUES(?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [key, value],
        )?;
        Ok(())
    }
}

#[cfg(unix)]
fn restrict_dir(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
}

#[cfg(not(unix))]
fn restrict_dir(_dir: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reopen_does_not_rerun_migrations() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(dir.path()).unwrap();
        s.set_setting("k", "v").unwrap();
        drop(s);
        let s = Store::open(dir.path()).unwrap();
        assert_eq!(s.get_setting("k").unwrap().as_deref(), Some("v"));
        let n: i64 = s
            .conn()
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, MIGRATIONS.len() as i64);
    }
}
