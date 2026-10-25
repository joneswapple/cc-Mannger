//! Upstream Claude Pro/Max OAuth accounts. Metadata is plaintext; tokens are
//! sealed with [`SecretBox`] before they reach SQLite.

use std::sync::Arc;

use anyhow::{bail, Context, Result};
use rusqlite::{params, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use crate::secret::SecretBox;
use crate::store::Store;

pub const STATUS_ACTIVE: &str = "active";
pub const STATUS_NEEDS_REAUTH: &str = "needs_reauth";

#[derive(Debug, thiserror::Error)]
#[error("account not found")]
pub struct NotFound;

#[derive(Debug, Clone, Default, Serialize)]
pub struct Account {
    pub id: i64,
    pub label: String,
    pub email: String,
    pub org_uuid: String,
    pub plan: String,
    pub priority: i64,
    pub weight: i64,
    pub enabled: bool,
    pub status: String,
    pub status_reason: String,
    pub proxy_url: String,
    pub source_path: String,
    pub sync_file: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

/// One OAuth token set. Field names match the Claude Code CLI credentials
/// file (`claudeAiOauth`), so the same struct reads and writes that file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Credentials {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: String,
    /// Unix milliseconds; 0 when unknown.
    #[serde(default)]
    pub expires_at: i64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scopes: Vec<String>,
}

#[derive(Clone)]
pub struct AccountStore {
    store: Store,
    secrets: Arc<SecretBox>,
}

const COLS: &str = "id, label, email, org_uuid, plan, priority, weight, enabled, status, \
    status_reason, proxy_url, source_path, sync_file, created_at, updated_at";

fn from_row(r: &Row<'_>) -> rusqlite::Result<Account> {
    Ok(Account {
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
        created_at: r.get(13)?,
        updated_at: r.get(14)?,
    })
}

/// access_token_enc, refresh_token_enc, expires_at, scopes
type SecretRow = (Vec<u8>, Option<Vec<u8>>, i64, String);

fn aad(id: i64, field: &str) -> Vec<u8> {
    format!("account:{id}:{field}").into_bytes()
}

impl AccountStore {
    pub fn new(store: Store, secrets: Arc<SecretBox>) -> Self {
        Self { store, secrets }
    }

    /// Inserts an account and its sealed credentials in one transaction.
    pub fn create(&self, mut a: Account, c: &Credentials) -> Result<i64> {
        if a.label.is_empty() {
            bail!("account label is required");
        }
        if c.access_token.is_empty() {
            bail!("account access token is required");
        }
        if a.weight <= 0 {
            a.weight = 1;
        }
        if a.status.is_empty() {
            a.status = STATUS_ACTIVE.into();
        }
        let now = crate::now_unix();
        let mut conn = self.store.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO accounts(label, email, org_uuid, plan, priority, weight, enabled, status,
                status_reason, proxy_url, source_path, sync_file, created_at, updated_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?13)",
            params![
                a.label,
                a.email,
                a.org_uuid,
                a.plan,
                a.priority,
                a.weight,
                a.enabled,
                a.status,
                a.status_reason,
                a.proxy_url,
                a.source_path,
                a.sync_file,
                now
            ],
        )?;
        let id = tx.last_insert_rowid();
        self.save_credentials_in(&tx, id, c)?;
        tx.commit()?;
        Ok(id)
    }

    pub fn get(&self, id: i64) -> Result<Account> {
        self.store
            .conn()
            .query_row(
                &format!("SELECT {COLS} FROM accounts WHERE id = ?1"),
                [id],
                from_row,
            )
            .optional()?
            .ok_or_else(|| NotFound.into())
    }

    /// All accounts in scheduling order: highest priority first, then oldest.
    pub fn list(&self) -> Result<Vec<Account>> {
        let conn = self.store.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLS} FROM accounts ORDER BY priority DESC, id ASC"
        ))?;
        let rows = stmt.query_map([], from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn count(&self) -> Result<i64> {
        Ok(self
            .store
            .conn()
            .query_row("SELECT COUNT(*) FROM accounts", [], |r| r.get(0))?)
    }

    pub fn set_status(&self, id: i64, status: &str, reason: &str) -> Result<()> {
        self.exec(
            "UPDATE accounts SET status = ?1, status_reason = ?2, updated_at = ?3 WHERE id = ?4",
            params![status, reason, crate::now_unix(), id],
        )
    }

    pub fn set_enabled(&self, id: i64, enabled: bool) -> Result<()> {
        self.exec(
            "UPDATE accounts SET enabled = ?1, updated_at = ?2 WHERE id = ?3",
            params![enabled, crate::now_unix(), id],
        )
    }

    pub fn delete(&self, id: i64) -> Result<()> {
        self.exec("DELETE FROM accounts WHERE id = ?1", params![id])
    }

    fn exec(&self, sql: &str, p: impl rusqlite::Params) -> Result<()> {
        if self.store.conn().execute(sql, p)? == 0 {
            return Err(NotFound.into());
        }
        Ok(())
    }

    /// Decrypts the stored token set for an account.
    pub fn load_credentials(&self, id: i64) -> Result<Credentials> {
        let row: Option<SecretRow> = self
            .store
            .conn()
            .query_row(
                "SELECT access_token_enc, refresh_token_enc, expires_at, scopes
                 FROM account_secrets WHERE account_id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        let (access, refresh, expires_at, scopes) = row.ok_or(NotFound)?;
        let open = |sealed: &[u8], field: &str| -> Result<String> {
            let plain = self
                .secrets
                .open(sealed, &aad(id, field))
                .with_context(|| format!("account {id} {field} token"))?;
            Ok(String::from_utf8(plain)?)
        };
        Ok(Credentials {
            access_token: open(&access, "access")?,
            refresh_token: match refresh {
                Some(r) if !r.is_empty() => open(&r, "refresh")?,
                _ => String::new(),
            },
            expires_at,
            scopes: scopes.split_whitespace().map(String::from).collect(),
        })
    }

    /// Replaces the stored token set for an account.
    pub fn save_credentials(&self, id: i64, c: &Credentials) -> Result<()> {
        let conn = self.store.conn();
        self.save_credentials_in(&conn, id, c)
    }

    fn save_credentials_in(
        &self,
        conn: &rusqlite::Connection,
        id: i64,
        c: &Credentials,
    ) -> Result<()> {
        let access = self
            .secrets
            .seal(c.access_token.as_bytes(), &aad(id, "access"))?;
        let refresh = if c.refresh_token.is_empty() {
            None
        } else {
            Some(
                self.secrets
                    .seal(c.refresh_token.as_bytes(), &aad(id, "refresh"))?,
            )
        };
        conn.execute(
            "INSERT INTO account_secrets(account_id, access_token_enc, refresh_token_enc, expires_at, scopes, updated_at)
             VALUES(?1,?2,?3,?4,?5,?6)
             ON CONFLICT(account_id) DO UPDATE SET
                access_token_enc = excluded.access_token_enc,
                refresh_token_enc = excluded.refresh_token_enc,
                expires_at = excluded.expires_at,
                scopes = excluded.scopes,
                updated_at = excluded.updated_at",
            params![id, access, refresh, c.expires_at, c.scopes.join(" "), crate::now_unix()],
        )?;
        Ok(())
    }
}

#[cfg(test)]
pub(crate) fn test_store() -> AccountStore {
    AccountStore::new(
        Store::open_in_memory().unwrap(),
        Arc::new(SecretBox::new(&[9u8; 32]).unwrap()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_list_and_credentials_round_trip() {
        let s = test_store();
        let creds = Credentials {
            access_token: "acc".into(),
            refresh_token: "ref".into(),
            expires_at: 42,
            scopes: vec!["user:inference".into()],
        };
        let low = s
            .create(
                Account {
                    label: "low".into(),
                    enabled: true,
                    ..Default::default()
                },
                &creds,
            )
            .unwrap();
        let high = s
            .create(
                Account {
                    label: "high".into(),
                    enabled: true,
                    priority: 5,
                    ..Default::default()
                },
                &creds,
            )
            .unwrap();
        let ids: Vec<i64> = s.list().unwrap().iter().map(|a| a.id).collect();
        assert_eq!(ids, vec![high, low]);
        assert_eq!(s.load_credentials(low).unwrap(), creds);

        let raw: Vec<u8> = s
            .store
            .conn()
            .query_row(
                "SELECT access_token_enc FROM account_secrets WHERE account_id = ?1",
                [low],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            !raw.windows(3).any(|w| w == b"acc"),
            "token stored in plaintext"
        );

        s.delete(low).unwrap();
        assert!(s.get(low).unwrap_err().is::<NotFound>());
        assert!(s.load_credentials(low).unwrap_err().is::<NotFound>());
    }
}
