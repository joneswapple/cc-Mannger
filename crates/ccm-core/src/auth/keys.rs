use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use rand::RngCore;
use rusqlite::{params, OptionalExtension};
use serde::Serialize;

use super::{sha256_hex, BCRYPT_COST};
use crate::store::Store;

pub const KEY_PREFIX: &str = "sk-cp-";
const KEY_BODY_BYTES: usize = 24;
/// Indexed prefix length for new keys ("sk-cp-" + 6 chars).
const PREFIX_LEN: usize = 12;
/// Prefix length used by keys issued by the Go cliproxy.
const LEGACY_PREFIX_LEN: usize = 8;
const CACHE_TTL: Duration = Duration::from_secs(60);

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum KeyError {
    #[error("invalid api key")]
    Invalid,
    #[error("api key revoked")]
    Revoked,
    #[error("user disabled")]
    UserDisabled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub user_id: i64,
    pub user_label: String,
    pub key_id: i64,
    pub key_prefix: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct UserKeys {
    pub id: i64,
    pub label: String,
    pub enabled: bool,
    pub created_at: i64,
    pub keys: Vec<KeyInfo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct KeyInfo {
    pub id: i64,
    pub prefix: String,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
    pub revoked_at: Option<i64>,
}

/// Terminal API keys. Lookups are cached for a minute, keyed by the key's
/// sha256 so plaintext keys never sit in memory longer than a request.
#[derive(Clone)]
pub struct Keys {
    store: Store,
    cache: Arc<Mutex<HashMap<String, (Identity, Instant)>>>,
}

pub fn generate_key() -> String {
    let mut b = [0u8; KEY_BODY_BYTES];
    rand::thread_rng().fill_bytes(&mut b);
    // Lowercase unpadded base32, same alphabet as the Go cliproxy keys.
    let body = base32_lower(&b);
    format!("{KEY_PREFIX}{body}")
}

fn base32_lower(data: &[u8]) -> String {
    const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
    let mut out = String::new();
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for &byte in data {
        buffer = (buffer << 8) | byte as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((buffer >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

fn prefix(full: &str, n: usize) -> &str {
    full.get(..n).unwrap_or(full)
}

impl Keys {
    pub fn new(store: Store) -> Self {
        Self {
            store,
            cache: Default::default(),
        }
    }

    pub fn create_user(&self, label: &str) -> Result<i64> {
        let label = label.trim();
        if label.is_empty() {
            bail!("label required");
        }
        let conn = self.store.conn();
        conn.execute(
            "INSERT INTO users(label, created_at, enabled) VALUES(?1, ?2, 1)",
            params![label, crate::now_unix()],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Issues a new key for a user. Returns (plaintext key, key id); the
    /// plaintext is shown exactly once.
    pub fn issue(&self, user_id: i64) -> Result<(String, i64)> {
        let full = generate_key();
        let hash = bcrypt::hash(&full, BCRYPT_COST)?;
        let id = insert_key(&self.store.conn(), user_id, &full, &hash)?;
        Ok((full, id))
    }

    /// Resolves a plaintext key to its identity. CPU-heavy on a cache miss
    /// (bcrypt); call from a blocking context.
    pub fn lookup(&self, full: &str) -> Result<Identity, KeyError> {
        if !full.starts_with(KEY_PREFIX) {
            return Err(KeyError::Invalid);
        }
        let digest = sha256_hex(full);
        {
            let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((id, expiry)) = cache.get(&digest) {
                if Instant::now() < *expiry {
                    return Ok(id.clone());
                }
                cache.remove(&digest);
            }
        }
        let rows = self.candidates(full).map_err(|e| {
            tracing::error!("key lookup: {e:#}");
            KeyError::Invalid
        })?;
        for c in rows {
            if !bcrypt::verify(full, &c.hash).unwrap_or(false) {
                continue;
            }
            if c.revoked {
                return Err(KeyError::Revoked);
            }
            if !c.user_enabled {
                return Err(KeyError::UserDisabled);
            }
            let id = Identity {
                user_id: c.user_id,
                user_label: c.user_label,
                key_id: c.key_id,
                key_prefix: c.prefix,
            };
            self.cache
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(digest, (id.clone(), Instant::now() + CACHE_TTL));
            let _ = self.store.conn().execute(
                "UPDATE api_keys SET last_used_at = ?1 WHERE id = ?2",
                params![crate::now_unix(), c.key_id],
            );
            return Ok(id);
        }
        Err(KeyError::Invalid)
    }

    fn candidates(&self, full: &str) -> Result<Vec<Candidate>> {
        let conn = self.store.conn();
        let mut stmt = conn.prepare(
            "SELECT api_keys.id, api_keys.prefix, api_keys.hash, api_keys.revoked_at IS NOT NULL,
                    users.id, users.label, users.enabled
             FROM api_keys JOIN users ON users.id = api_keys.user_id
             WHERE api_keys.prefix IN (?1, ?2)",
        )?;
        let rows = stmt.query_map(
            params![prefix(full, PREFIX_LEN), prefix(full, LEGACY_PREFIX_LEN)],
            |r| {
                Ok(Candidate {
                    key_id: r.get(0)?,
                    prefix: r.get(1)?,
                    hash: r.get(2)?,
                    revoked: r.get(3)?,
                    user_id: r.get(4)?,
                    user_label: r.get(5)?,
                    user_enabled: r.get(6)?,
                })
            },
        )?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn revoke(&self, key_id: i64) -> Result<()> {
        self.store.conn().execute(
            "UPDATE api_keys SET revoked_at = ?1 WHERE id = ?2 AND revoked_at IS NULL",
            params![crate::now_unix(), key_id],
        )?;
        self.clear_cache();
        Ok(())
    }

    pub fn set_user_enabled(&self, user_id: i64, enabled: bool) -> Result<()> {
        self.store.conn().execute(
            "UPDATE users SET enabled = ?1 WHERE id = ?2",
            params![enabled, user_id],
        )?;
        self.clear_cache();
        Ok(())
    }

    fn clear_cache(&self) {
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    pub fn user_exists(&self, user_id: i64) -> Result<bool> {
        Ok(self
            .store
            .conn()
            .query_row("SELECT 1 FROM users WHERE id = ?1", [user_id], |_| Ok(()))
            .optional()?
            .is_some())
    }

    pub fn list_users(&self) -> Result<Vec<UserKeys>> {
        let conn = self.store.conn();
        let mut users: Vec<UserKeys> = conn
            .prepare("SELECT id, label, enabled, created_at FROM users ORDER BY id")?
            .query_map([], |r| {
                Ok(UserKeys {
                    id: r.get(0)?,
                    label: r.get(1)?,
                    enabled: r.get(2)?,
                    created_at: r.get(3)?,
                    keys: vec![],
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        let mut stmt = conn.prepare(
            "SELECT id, prefix, created_at, last_used_at, revoked_at FROM api_keys
             WHERE user_id = ?1 ORDER BY id",
        )?;
        for u in &mut users {
            u.keys = stmt
                .query_map([u.id], |r| {
                    Ok(KeyInfo {
                        id: r.get(0)?,
                        prefix: r.get(1)?,
                        created_at: r.get(2)?,
                        last_used_at: r.get(3)?,
                        revoked_at: r.get(4)?,
                    })
                })?
                .collect::<rusqlite::Result<_>>()?;
        }
        Ok(users)
    }
}

struct Candidate {
    key_id: i64,
    prefix: String,
    hash: String,
    revoked: bool,
    user_id: i64,
    user_label: String,
    user_enabled: bool,
}

pub(crate) fn insert_key(
    conn: &rusqlite::Connection,
    user_id: i64,
    full: &str,
    hash: &str,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO api_keys(user_id, prefix, hash, created_at) VALUES(?1, ?2, ?3, ?4)",
        params![user_id, prefix(full, PREFIX_LEN), hash, crate::now_unix()],
    )?;
    Ok(conn.last_insert_rowid())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys() -> Keys {
        Keys::new(Store::open_in_memory().unwrap())
    }

    #[test]
    fn issue_and_lookup() {
        let k = keys();
        let uid = k.create_user("alice").unwrap();
        let (full, kid) = k.issue(uid).unwrap();
        assert!(full.starts_with(KEY_PREFIX));
        assert_eq!(full.len(), KEY_PREFIX.len() + 39);
        let id = k.lookup(&full).unwrap();
        assert_eq!(
            (id.user_id, id.key_id, id.user_label.as_str()),
            (uid, kid, "alice")
        );
        assert_eq!(k.lookup("sk-cp-bogus"), Err(KeyError::Invalid));
        assert_eq!(k.lookup("not-a-key"), Err(KeyError::Invalid));
    }

    #[test]
    fn revoke_and_disable() {
        let k = keys();
        let uid = k.create_user("alice").unwrap();
        let (full, kid) = k.issue(uid).unwrap();
        k.lookup(&full).unwrap();
        k.set_user_enabled(uid, false).unwrap();
        assert_eq!(k.lookup(&full), Err(KeyError::UserDisabled));
        k.set_user_enabled(uid, true).unwrap();
        k.revoke(kid).unwrap();
        assert_eq!(k.lookup(&full), Err(KeyError::Revoked));
    }

    #[test]
    fn legacy_go_keys_still_resolve() {
        let k = keys();
        let uid = k.create_user("old").unwrap();
        let full = generate_key();
        let hash = bcrypt::hash(&full, 4).unwrap();
        k.store
            .conn()
            .execute(
                "INSERT INTO api_keys(user_id, prefix, hash, created_at) VALUES(?1, ?2, ?3, 0)",
                params![uid, &full[..LEGACY_PREFIX_LEN], hash],
            )
            .unwrap();
        assert_eq!(k.lookup(&full).unwrap().user_id, uid);
    }

    #[test]
    fn many_keys_do_not_collide() {
        let k = keys();
        let uid = k.create_user("bulk").unwrap();
        for _ in 0..60 {
            let full = generate_key();
            insert_key(&k.store.conn(), uid, &full, "x").unwrap();
        }
    }
}
