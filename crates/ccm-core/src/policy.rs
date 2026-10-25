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
type PolicyRow = (
    Option<String>,
    Option<i64>,
    Option<i64>,
    Option<String>,
    Option<i64>,
);

/// Window in which distinct client IPs count against `max_ips`.
pub const IP_WINDOW_SECS: i64 = 86_400;

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
    #[error("this terminal key was already used from {0} IP addresses in the last 24 hours, the most its policy allows")]
    TooManyIps(i64),
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
    /// Distinct client IPs allowed per 24 hours.
    #[serde(default)]
    pub max_ips: Option<i64>,
}

/// One client IP a terminal has used.
#[derive(Debug, Clone, Serialize)]
pub struct UserIp {
    pub ip: String,
    pub first_seen: i64,
    pub last_seen: i64,
    pub requests: i64,
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
                "SELECT allowed_models, daily_token_limit, rpm_limit, account_group, max_ips
                 FROM policies WHERE user_id = ?1",
                [user_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()?;
        let Some((allowed, daily, rpm, group, max_ips)) = row else {
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
            max_ips,
        })
    }

    pub fn set_policy(&self, user_id: i64, p: &Policy) -> Result<()> {
        let allowed = p
            .allowed_models
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        self.store.conn().execute(
            "INSERT INTO policies(user_id, allowed_models, daily_token_limit, rpm_limit, account_group, max_ips)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(user_id) DO UPDATE SET
                allowed_models = excluded.allowed_models,
                daily_token_limit = excluded.daily_token_limit,
                rpm_limit = excluded.rpm_limit,
                account_group = excluded.account_group,
                max_ips = excluded.max_ips",
            params![
                user_id,
                allowed,
                p.daily_token_limit,
                p.rpm_limit,
                p.account_group.as_deref().map(str::trim).filter(|g| !g.is_empty()),
                p.max_ips.filter(|n| *n > 0),
            ],
        )?;
        Ok(())
    }

    /// Records that `user_id` called from `ip`, unless that would exceed
    /// `max_ips` distinct IPs within [`IP_WINDOW_SECS`]. An IP already seen
    /// in the window is always allowed. Empty IPs (in-process calls) are
    /// not tracked.
    pub fn track_ip(
        &self,
        user_id: i64,
        ip: &str,
        max_ips: Option<i64>,
    ) -> Result<Result<(), Denied>> {
        if ip.is_empty() {
            return Ok(Ok(()));
        }
        let now = crate::now_unix();
        let conn = self.store.conn();
        if let Some(limit) = max_ips.filter(|n| *n > 0) {
            let since = now - IP_WINDOW_SECS;
            let known: bool = conn
                .query_row(
                    "SELECT 1 FROM user_ips WHERE user_id = ?1 AND ip = ?2 AND last_seen >= ?3",
                    params![user_id, ip, since],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            if !known {
                let used: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM user_ips WHERE user_id = ?1 AND last_seen >= ?2",
                    params![user_id, since],
                    |r| r.get(0),
                )?;
                if used >= limit {
                    return Ok(Err(Denied::TooManyIps(used)));
                }
            }
        }
        conn.execute(
            "INSERT INTO user_ips(user_id, ip, first_seen, last_seen, requests) VALUES(?1, ?2, ?3, ?3, 1)
             ON CONFLICT(user_id, ip) DO UPDATE SET last_seen = excluded.last_seen, requests = requests + 1",
            params![user_id, ip, now],
        )?;
        Ok(Ok(()))
    }

    /// IPs a terminal has used, most recent first.
    pub fn user_ips(&self, user_id: i64) -> Result<Vec<UserIp>> {
        let conn = self.store.conn();
        let mut stmt = conn.prepare(
            "SELECT ip, first_seen, last_seen, requests FROM user_ips
             WHERE user_id = ?1 ORDER BY last_seen DESC LIMIT 200",
        )?;
        let rows = stmt.query_map([user_id], |r| {
            Ok(UserIp {
                ip: r.get(0)?,
                first_seen: r.get(1)?,
                last_seen: r.get(2)?,
                requests: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
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
