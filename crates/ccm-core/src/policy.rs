//! Request-time gate: global model denylist, per-user model allowlist,
//! in-memory RPM bucket, and DB-backed daily token quota.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::{Local, TimeZone};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::store::Store;

const SETTING_DISABLED_MODELS: &str = "disabled_models";

/// allowed_models, daily_token_limit, rpm_limit, account_group
type PolicyRow = (Option<String>, Option<i64>, Option<i64>, Option<String>);

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Denied {
    #[error("model disabled by global policy")]
    ModelDisabled,
    #[error("model not allowed for this user")]
    ModelNotAllowed,
    #[error("rate limit exceeded")]
    RateLimited,
    #[error("daily token quota exceeded")]
    QuotaExceeded,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Policy {
    /// `None` allows every model.
    pub allowed_models: Option<Vec<String>>,
    pub daily_token_limit: Option<i64>,
    pub rpm_limit: Option<i64>,
    /// Restricts the user to upstream accounts in this group.
    #[serde(default)]
    pub account_group: Option<String>,
}

#[derive(Clone)]
pub struct Engine {
    store: Store,
    buckets: Arc<Mutex<HashMap<i64, (u32, Instant)>>>,
}

impl Engine {
    pub fn new(store: Store) -> Self {
        Self {
            store,
            buckets: Default::default(),
        }
    }

    pub fn get_policy(&self, user_id: i64) -> Result<Policy> {
        let row: Option<PolicyRow> = self
            .store
            .conn()
            .query_row(
                "SELECT allowed_models, daily_token_limit, rpm_limit, account_group
                 FROM policies WHERE user_id = ?1",
                [user_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        let Some((allowed, daily, rpm, group)) = row else {
            return Ok(Policy::default());
        };
        Ok(Policy {
            allowed_models: match allowed.as_deref() {
                None | Some("") => None,
                Some(s) => Some(serde_json::from_str(s)?),
            },
            daily_token_limit: daily,
            rpm_limit: rpm,
            account_group: group.filter(|g| !g.is_empty()),
        })
    }

    pub fn set_policy(&self, user_id: i64, p: &Policy) -> Result<()> {
        let allowed = p
            .allowed_models
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        self.store.conn().execute(
            "INSERT INTO policies(user_id, allowed_models, daily_token_limit, rpm_limit, account_group)
             VALUES(?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(user_id) DO UPDATE SET
                allowed_models = excluded.allowed_models,
                daily_token_limit = excluded.daily_token_limit,
                rpm_limit = excluded.rpm_limit,
                account_group = excluded.account_group",
            params![
                user_id,
                allowed,
                p.daily_token_limit,
                p.rpm_limit,
                p.account_group.as_deref().map(str::trim).filter(|g| !g.is_empty())
            ],
        )?;
        Ok(())
    }

    pub fn disabled_models(&self) -> Result<Vec<String>> {
        match self.store.get_setting(SETTING_DISABLED_MODELS)? {
            Some(v) if !v.is_empty() => Ok(serde_json::from_str(&v)?),
            _ => Ok(vec![]),
        }
    }

    pub fn set_disabled_models(&self, models: &[String]) -> Result<()> {
        self.store
            .set_setting(SETTING_DISABLED_MODELS, &serde_json::to_string(models)?)
    }

    /// Evaluates every gate for one request and returns the user's policy
    /// when allowed. `Ok(Err(_))` is a policy denial; the outer error is a
    /// storage failure.
    pub fn check(&self, user_id: i64, model: &str) -> Result<Result<Policy, Denied>> {
        if self
            .disabled_models()?
            .iter()
            .any(|m| m.eq_ignore_ascii_case(model))
        {
            return Ok(Err(Denied::ModelDisabled));
        }
        let p = self.get_policy(user_id)?;
        if let Some(allowed) = &p.allowed_models {
            if !allowed.iter().any(|m| m.eq_ignore_ascii_case(model)) {
                return Ok(Err(Denied::ModelNotAllowed));
            }
        }
        if let Some(limit) = p.rpm_limit {
            if !self.allow_rpm(user_id, limit) {
                return Ok(Err(Denied::RateLimited));
            }
        }
        if let Some(limit) = p.daily_token_limit {
            if self.daily_tokens_used(user_id)? >= limit {
                return Ok(Err(Denied::QuotaExceeded));
            }
        }
        Ok(Ok(p))
    }

    /// Fixed one-minute window per user.
    fn allow_rpm(&self, user_id: i64, limit: i64) -> bool {
        let mut buckets = self.buckets.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        match buckets.get_mut(&user_id) {
            Some((count, start)) if now.duration_since(*start) < Duration::from_secs(60) => {
                if i64::from(*count) >= limit {
                    return false;
                }
                *count += 1;
                true
            }
            _ => {
                buckets.insert(user_id, (1, now));
                true
            }
        }
    }

    /// Input + output tokens since local midnight.
    fn daily_tokens_used(&self, user_id: i64) -> Result<i64> {
        let today = Local::now().date_naive().and_hms_opt(0, 0, 0).unwrap();
        let start = Local
            .from_local_datetime(&today)
            .earliest()
            .map(|t| t.timestamp())
            .unwrap_or(0);
        Ok(self.store.conn().query_row(
            "SELECT COALESCE(SUM(input_tokens + output_tokens), 0)
             FROM usage_events WHERE user_id = ?1 AND ts >= ?2",
            params![user_id, start],
            |r| r.get(0),
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::Keys;

    fn setup() -> (Engine, Store, i64) {
        let store = Store::open_in_memory().unwrap();
        let uid = Keys::new(store.clone()).create_user("alice").unwrap();
        (Engine::new(store.clone()), store, uid)
    }

    #[test]
    fn model_allowlist_is_case_insensitive() {
        let (e, _, uid) = setup();
        let p = Policy {
            allowed_models: Some(vec!["Claude-Sonnet-4-5".into()]),
            ..Default::default()
        };
        e.set_policy(uid, &p).unwrap();
        assert_eq!(e.get_policy(uid).unwrap(), p);
        assert_eq!(
            e.check(uid, "claude-opus-4-1").unwrap().map(|_| ()),
            Err(Denied::ModelNotAllowed)
        );
        assert_eq!(
            e.check(uid, "claude-sonnet-4-5").unwrap().map(|_| ()),
            Ok(())
        );
    }

    #[test]
    fn globally_disabled_model() {
        let (e, _, uid) = setup();
        e.set_disabled_models(&["claude-opus-4-1".into()]).unwrap();
        assert_eq!(
            e.check(uid, "CLAUDE-OPUS-4-1").unwrap().map(|_| ()),
            Err(Denied::ModelDisabled)
        );
        assert_eq!(e.check(uid, "claude-haiku").unwrap().map(|_| ()), Ok(()));
    }

    #[test]
    fn rpm() {
        let (e, _, uid) = setup();
        e.set_policy(
            uid,
            &Policy {
                rpm_limit: Some(2),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(e.check(uid, "m").unwrap().map(|_| ()), Ok(()));
        assert_eq!(e.check(uid, "m").unwrap().map(|_| ()), Ok(()));
        assert_eq!(
            e.check(uid, "m").unwrap().map(|_| ()),
            Err(Denied::RateLimited)
        );
    }

    #[test]
    fn daily_quota() {
        let (e, store, uid) = setup();
        e.set_policy(
            uid,
            &Policy {
                daily_token_limit: Some(100),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(e.check(uid, "m").unwrap().map(|_| ()), Ok(()));
        store
            .conn()
            .execute(
                "INSERT INTO usage_events(ts, user_id, key_id, model, input_tokens, output_tokens, latency_ms, status)
                 VALUES(?1, ?2, 0, 'm', 60, 60, 0, 200)",
                params![crate::now_unix(), uid],
            )
            .unwrap();
        assert_eq!(
            e.check(uid, "m").unwrap().map(|_| ()),
            Err(Denied::QuotaExceeded)
        );
    }
}
