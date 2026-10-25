//! Upstream Claude Pro/Max OAuth accounts. Metadata is plaintext; tokens are
//! sealed with [`SecretBox`] before they reach SQLite.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use rusqlite::{params, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use crate::events::{CoreEvent, Events};
use crate::quota::{Quota, QuotaSnapshot};
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
    /// Unix seconds until which the scheduler skips this account (0 = none).
    pub cooldown_until: i64,
    pub account_group: String,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Account {
    /// Seconds of cooldown left at `now` (unix seconds).
    pub fn cooldown_left(&self, now: i64) -> i64 {
        (self.cooldown_until - now).max(0)
    }
}

/// Fields an admin may change on an existing account; `None` keeps the
/// current value.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AccountPatch {
    pub label: Option<String>,
    pub priority: Option<i64>,
    pub weight: Option<i64>,
    pub enabled: Option<bool>,
    pub account_group: Option<String>,
    /// Outbound HTTP / SOCKS5 proxy for this account; empty string removes it.
    pub proxy_url: Option<String>,
    /// `true` lifts an active cooldown.
    #[serde(default)]
    pub clear_cooldown: bool,
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
    events: Events,
}

const COLS: &str = "id, label, email, org_uuid, plan, priority, weight, enabled, status, \
    status_reason, proxy_url, source_path, sync_file, cooldown_until, account_group, created_at, updated_at";

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
        cooldown_until: r.get(13)?,
        account_group: r.get(14)?,
        created_at: r.get(15)?,
        updated_at: r.get(16)?,
    })
}

/// access_token_enc, refresh_token_enc, expires_at, scopes
type SecretRow = (Vec<u8>, Option<Vec<u8>>, i64, String);

/// Accepts `http(s)://` and `socks5(h)://` proxy URLs; empty clears.
pub fn validate_proxy_url(s: &str) -> Result<String> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(String::new());
    }
    let url = reqwest::Url::parse(s).with_context(|| format!("invalid proxy url {s:?}"))?;
    if !matches!(url.scheme(), "http" | "https" | "socks5" | "socks5h") || url.host().is_none() {
        bail!("proxy url must be http://, https://, socks5:// or socks5h:// with a host");
    }
    Ok(s.to_string())
}

fn aad(id: i64, field: &str) -> Vec<u8> {
    format!("account:{id}:{field}").into_bytes()
}

impl AccountStore {
    pub fn new(store: Store, secrets: Arc<SecretBox>) -> Self {
        Self {
            store,
            secrets,
            events: Events::default(),
        }
    }

    /// Health changes made through this store (needs_reauth, cooldowns,
    /// quota updates) are broadcast here.
    pub fn events(&self) -> &Events {
        &self.events
    }

    /// Merges a quota reading into the stored snapshot; fields the reading
    /// lacks keep their previous value.
    pub fn save_quota(&self, id: i64, q: &QuotaSnapshot) -> Result<()> {
        if q.is_empty() {
            return Ok(());
        }
        self.store.conn().execute(
            "INSERT INTO account_quota(account_id, five_hour_utilization, five_hour_reset,
                seven_day_utilization, seven_day_reset, status, updated_at)
             VALUES(?1, ?2, ?3, ?4, ?5, COALESCE(?6, ''), ?7)
             ON CONFLICT(account_id) DO UPDATE SET
                five_hour_utilization = COALESCE(excluded.five_hour_utilization, five_hour_utilization),
                five_hour_reset = COALESCE(?3, five_hour_reset),
                seven_day_utilization = COALESCE(excluded.seven_day_utilization, seven_day_utilization),
                seven_day_reset = COALESCE(?5, seven_day_reset),
                status = COALESCE(?6, status),
                updated_at = excluded.updated_at",
            params![
                id,
                q.five_hour_utilization,
                q.five_hour_reset,
                q.seven_day_utilization,
                q.seven_day_reset,
                q.status,
                crate::now_unix()
            ],
        )?;
        self.events.send(CoreEvent::QuotaUpdated { account_id: id });
        Ok(())
    }

    /// Stored quota per account.
    pub fn quotas(&self) -> Result<HashMap<i64, Quota>> {
        let conn = self.store.conn();
        let mut stmt = conn.prepare(
            "SELECT account_id, five_hour_utilization, five_hour_reset, seven_day_utilization,
                    seven_day_reset, status, updated_at FROM account_quota",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get(0)?,
                Quota {
                    five_hour_utilization: r.get(1)?,
                    five_hour_reset: r.get(2)?,
                    seven_day_utilization: r.get(3)?,
                    seven_day_reset: r.get(4)?,
                    status: r.get(5)?,
                    updated_at: r.get(6)?,
                },
            ))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
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
                status_reason, proxy_url, source_path, sync_file, account_group, created_at, updated_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?14)",
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
                a.account_group,
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
        let before = self.get(id)?.status;
        self.exec(
            "UPDATE accounts SET status = ?1, status_reason = ?2, updated_at = ?3 WHERE id = ?4",
            params![status, reason, crate::now_unix(), id],
        )?;
        if status == STATUS_NEEDS_REAUTH && before != STATUS_NEEDS_REAUTH {
            self.events.send(CoreEvent::NeedsReauth {
                account_id: id,
                reason: reason.to_string(),
            });
        }
        Ok(())
    }

    pub fn set_enabled(&self, id: i64, enabled: bool) -> Result<()> {
        self.exec(
            "UPDATE accounts SET enabled = ?1, updated_at = ?2 WHERE id = ?3",
            params![enabled, crate::now_unix(), id],
        )
    }

    /// Parks an account until `until` (unix seconds) with a reason shown to
    /// admins.
    pub fn set_cooldown(&self, id: i64, until: i64, reason: &str) -> Result<()> {
        self.exec(
            "UPDATE accounts SET cooldown_until = ?1, status_reason = ?2, updated_at = ?3 WHERE id = ?4",
            params![until, reason, crate::now_unix(), id],
        )?;
        self.events.send(CoreEvent::CoolingDown {
            account_id: id,
            seconds: until - crate::now_unix(),
            reason: reason.to_string(),
        });
        Ok(())
    }

    pub fn update(&self, id: i64, p: &AccountPatch) -> Result<Account> {
        let mut a = self.get(id)?;
        if let Some(label) = &p.label {
            if label.trim().is_empty() {
                bail!("account label is required");
            }
            a.label = label.trim().to_string();
        }
        if let Some(v) = p.priority {
            a.priority = v;
        }
        if let Some(v) = p.weight {
            a.weight = v.max(1);
        }
        if let Some(v) = p.enabled {
            a.enabled = v;
        }
        if let Some(g) = &p.account_group {
            a.account_group = g.trim().to_string();
        }
        if let Some(proxy) = &p.proxy_url {
            a.proxy_url = validate_proxy_url(proxy)?;
        }
        if p.clear_cooldown {
            a.cooldown_until = 0;
            // The reason belonged to the cooldown; a needs_reauth reason stays.
            if a.status == STATUS_ACTIVE {
                a.status_reason.clear();
            }
        }
        self.exec(
            "UPDATE accounts SET label = ?1, priority = ?2, weight = ?3, enabled = ?4,
                account_group = ?5, cooldown_until = ?6, status_reason = ?7, proxy_url = ?8,
                updated_at = ?9
             WHERE id = ?10",
            params![
                a.label,
                a.priority,
                a.weight,
                a.enabled,
                a.account_group,
                a.cooldown_until,
                a.status_reason,
                a.proxy_url,
                crate::now_unix(),
                id
            ],
        )?;
        self.get(id)
    }

    /// The account already holding this email, if any (re-logins update it
    /// instead of adding a duplicate).
    pub fn find_by_email(&self, email: &str) -> Result<Option<Account>> {
        if email.is_empty() {
            return Ok(None);
        }
        Ok(self
            .store
            .conn()
            .query_row(
                &format!("SELECT {COLS} FROM accounts WHERE email = ?1 ORDER BY id LIMIT 1"),
                [email],
                from_row,
            )
            .optional()?)
    }

    /// Records what the profile endpoint said; empty values keep the old one.
    pub fn set_profile(&self, id: i64, email: &str, org_uuid: &str, plan: &str) -> Result<()> {
        self.exec(
            "UPDATE accounts SET
                email = CASE WHEN ?1 = '' THEN email ELSE ?1 END,
                org_uuid = CASE WHEN ?2 = '' THEN org_uuid ELSE ?2 END,
                plan = CASE WHEN ?3 = '' THEN plan ELSE ?3 END,
                updated_at = ?4
             WHERE id = ?5",
            params![email, org_uuid, plan, crate::now_unix(), id],
        )
    }

    /// One above the current highest priority, so `priority` scheduling
    /// prefers this account ("pin to top").
    pub fn pin(&self, id: i64) -> Result<Account> {
        let top: i64 = self.store.conn().query_row(
            "SELECT COALESCE(MAX(priority), 0) FROM accounts WHERE id != ?1",
            [id],
            |r| r.get(0),
        )?;
        self.update(
            id,
            &AccountPatch {
                priority: Some(top + 1),
                ..Default::default()
            },
        )
    }

    /// Input + output tokens per account since `since` (unix seconds).
    pub fn recent_tokens(&self, since: i64) -> Result<HashMap<i64, i64>> {
        let conn = self.store.conn();
        let mut stmt = conn.prepare(
            "SELECT account_id, COALESCE(SUM(input_tokens + output_tokens), 0)
             FROM usage_events WHERE ts >= ?1 AND account_id IS NOT NULL GROUP BY account_id",
        )?;
        let rows = stmt.query_map([since], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
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
impl AccountStore {
    pub(crate) fn store_for_tests(&self) -> &Store {
        &self.store
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
