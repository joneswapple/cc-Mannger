use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;

use super::{random_hex, sha256_hex, BCRYPT_COST};
use crate::store::Store;

const SETTING_KEY: &str = "admin_token_hash";
const VERIFY_CACHE_TTL: Duration = Duration::from_secs(60);

#[derive(Debug, thiserror::Error)]
#[error("unauthorized")]
pub struct AdminUnauthorized;

/// The admin token guarding `/admin/api/*`. Stored bcrypt-hashed in
/// `global_settings`, compatible with hashes written by the Go cliproxy.
#[derive(Clone)]
pub struct Admin {
    store: Store,
    /// sha256 of the last token that verified, so each admin request
    /// doesn't pay for a bcrypt check.
    verified: Arc<Mutex<Option<(String, Instant)>>>,
}

impl Admin {
    pub fn new(store: Store) -> Self {
        Self {
            store,
            verified: Default::default(),
        }
    }

    /// Ensures an admin token exists. A non-empty `preset` is (re)installed;
    /// otherwise a random token is generated if none exists. Returns the
    /// plaintext token when one was generated or installed, `None` when an
    /// existing token was kept.
    pub fn bootstrap(&self, preset: &str) -> Result<Option<String>> {
        if preset.is_empty() && self.store.get_setting(SETTING_KEY)?.is_some() {
            return Ok(None);
        }
        Ok(Some(self.reset(preset)?))
    }

    /// Installs a new admin token (random when `preset` is empty) and
    /// returns it.
    pub fn reset(&self, preset: &str) -> Result<String> {
        let token = if preset.is_empty() {
            random_hex(24)
        } else {
            preset.to_string()
        };
        self.store
            .set_setting(SETTING_KEY, &bcrypt::hash(&token, BCRYPT_COST)?)?;
        *self.verified.lock().unwrap_or_else(|e| e.into_inner()) = None;
        Ok(token)
    }

    /// Checks a presented token. CPU-heavy on a cache miss (bcrypt).
    pub fn verify(&self, token: &str) -> Result<()> {
        let token = token.trim();
        if token.is_empty() {
            return Err(AdminUnauthorized.into());
        }
        let digest = sha256_hex(token);
        if let Some((d, until)) = &*self.verified.lock().unwrap_or_else(|e| e.into_inner()) {
            if *d == digest && Instant::now() < *until {
                return Ok(());
            }
        }
        let hash = self
            .store
            .get_setting(SETTING_KEY)?
            .ok_or(AdminUnauthorized)?;
        if !bcrypt::verify(token, &hash).unwrap_or(false) {
            return Err(AdminUnauthorized.into());
        }
        *self.verified.lock().unwrap_or_else(|e| e.into_inner()) =
            Some((digest, Instant::now() + VERIFY_CACHE_TTL));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_generates_once_and_verifies() {
        let a = Admin::new(Store::open_in_memory().unwrap());
        let tok = a.bootstrap("").unwrap().unwrap();
        assert!(a.bootstrap("").unwrap().is_none());
        a.verify(&tok).unwrap();
        a.verify(&tok).unwrap(); // cached path
        assert!(a.verify("wrong").is_err());
        assert!(a.verify("").is_err());
    }

    #[test]
    fn preset_and_reset_replace_token() {
        let a = Admin::new(Store::open_in_memory().unwrap());
        a.bootstrap("first").unwrap();
        a.verify("first").unwrap();
        let new = a.reset("").unwrap();
        assert!(
            a.verify("first").is_err(),
            "old token must stop working after reset"
        );
        a.verify(&new).unwrap();
    }
}
