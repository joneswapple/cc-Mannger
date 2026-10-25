use anyhow::{bail, Result};
use rusqlite::{params, OptionalExtension};
use serde::Serialize;

use super::keys::{generate_key, insert_key};
use super::{random_hex, sha256_hex, BCRYPT_COST};
use crate::store::Store;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum InviteError {
    #[error("invite invalid")]
    Invalid,
    #[error("invite expired")]
    Expired,
    #[error("invite already redeemed")]
    Redeemed,
}

#[derive(Debug, Clone, Serialize)]
pub struct InviteInfo {
    pub token_hash: String,
    pub label: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub redeemed_at: Option<i64>,
    pub user_id: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct Redeemed {
    pub user_id: i64,
    pub api_key: String,
    pub key_id: i64,
}

/// Single-use invite tokens; only their sha256 is stored.
#[derive(Clone)]
pub struct Invites {
    store: Store,
}

impl Invites {
    pub fn new(store: Store) -> Self {
        Self { store }
    }

    /// Returns (plaintext token, expires_at). The token is shown once.
    pub fn create(&self, label: &str, ttl_secs: i64) -> Result<(String, i64)> {
        let label = label.trim();
        if label.is_empty() {
            bail!("label required");
        }
        let token = random_hex(24);
        let now = crate::now_unix();
        let expires = now + ttl_secs;
        self.store.conn().execute(
            "INSERT INTO invites(token_hash, label, created_at, expires_at) VALUES(?1, ?2, ?3, ?4)",
            params![sha256_hex(&token), label, now, expires],
        )?;
        Ok((token, expires))
    }

    pub fn list(&self) -> Result<Vec<InviteInfo>> {
        let conn = self.store.conn();
        let mut stmt = conn.prepare(
            "SELECT token_hash, label, created_at, expires_at, redeemed_at, redeemed_user_id
             FROM invites ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(InviteInfo {
                token_hash: r.get(0)?,
                label: r.get(1)?,
                created_at: r.get(2)?,
                expires_at: r.get(3)?,
                redeemed_at: r.get(4)?,
                user_id: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Deletes an unredeemed invite.
    pub fn delete_by_hash(&self, token_hash: &str) -> Result<()> {
        self.store.conn().execute(
            "DELETE FROM invites WHERE token_hash = ?1 AND redeemed_at IS NULL",
            [token_hash],
        )?;
        Ok(())
    }

    /// Redeems an invite: creates a user and its first key atomically.
    /// CPU-heavy (bcrypt); call from a blocking context.
    pub fn redeem(&self, token: &str, label: &str) -> Result<Redeemed> {
        let hash = sha256_hex(token.trim());
        let full = generate_key();
        let key_hash = bcrypt::hash(&full, BCRYPT_COST)?;
        let mut conn = self.store.conn();
        let tx = conn.transaction()?;
        let row: Option<(String, i64, Option<i64>)> = tx
            .query_row(
                "SELECT label, expires_at, redeemed_at FROM invites WHERE token_hash = ?1",
                [&hash],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let (invite_label, expires, redeemed) = row.ok_or(InviteError::Invalid)?;
        if redeemed.is_some() {
            return Err(InviteError::Redeemed.into());
        }
        let now = crate::now_unix();
        if now > expires {
            return Err(InviteError::Expired.into());
        }
        let label = match label.trim() {
            "" if !invite_label.is_empty() => invite_label,
            "" => "(unnamed)".to_string(),
            l => l.to_string(),
        };
        tx.execute(
            "INSERT INTO users(label, created_at, enabled) VALUES(?1, ?2, 1)",
            params![label, now],
        )?;
        let user_id = tx.last_insert_rowid();
        let key_id = insert_key(&tx, user_id, &full, &key_hash)?;
        tx.execute(
            "UPDATE invites SET redeemed_at = ?1, redeemed_user_id = ?2 WHERE token_hash = ?3",
            params![now, user_id, hash],
        )?;
        tx.commit()?;
        Ok(Redeemed {
            user_id,
            api_key: full,
            key_id,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::Keys;

    #[test]
    fn redeem_is_single_use_and_issues_working_key() {
        let store = Store::open_in_memory().unwrap();
        let inv = Invites::new(store.clone());
        let (tok, _) = inv.create("laptop", 3600).unwrap();
        let r = inv.redeem(&tok, "").unwrap();
        let id = Keys::new(store).lookup(&r.api_key).unwrap();
        assert_eq!((id.user_id, id.user_label.as_str()), (r.user_id, "laptop"));
        let err = inv.redeem(&tok, "x").unwrap_err();
        assert_eq!(
            err.downcast_ref::<InviteError>(),
            Some(&InviteError::Redeemed)
        );
        assert_eq!(inv.list().unwrap()[0].user_id, Some(r.user_id));
    }

    #[test]
    fn expired_and_invalid() {
        let inv = Invites::new(Store::open_in_memory().unwrap());
        let (tok, _) = inv.create("x", -1).unwrap();
        let err = inv.redeem(&tok, "").unwrap_err();
        assert_eq!(
            err.downcast_ref::<InviteError>(),
            Some(&InviteError::Expired)
        );
        let err = inv.redeem("nope", "").unwrap_err();
        assert_eq!(
            err.downcast_ref::<InviteError>(),
            Some(&InviteError::Invalid)
        );
    }

    #[test]
    fn delete_only_unredeemed() {
        let store = Store::open_in_memory().unwrap();
        let inv = Invites::new(store);
        let (a, _) = inv.create("a", 3600).unwrap();
        inv.create("b", 3600).unwrap();
        inv.redeem(&a, "").unwrap();
        for i in inv.list().unwrap() {
            inv.delete_by_hash(&i.token_hash).unwrap();
        }
        let left = inv.list().unwrap();
        assert_eq!(left.len(), 1);
        assert!(left[0].redeemed_at.is_some());
    }
}
