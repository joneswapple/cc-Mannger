//! Choosing the upstream account for each call, and parking accounts that
//! the upstream throttled or rejected.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::accounts::{Account, AccountStore, STATUS_ACTIVE};
use crate::config::{PoolConfig, Strategy};
use crate::oauth::OAuthClient;
use crate::provider::AccountProvider;

/// Window for the `least_used` strategy.
const LEAST_USED_WINDOW_SECS: i64 = 5 * 3600;

#[derive(Debug, thiserror::Error)]
#[error("no enabled upstream account — import one, or log in with `claude /login` on the host and restart")]
pub struct NoAccount;

/// Every eligible account is cooling down; the earliest is free again in
/// `retry_after` seconds.
#[derive(Debug, thiserror::Error)]
#[error("all upstream accounts are rate limited; retry in {retry_after}s")]
pub struct AllCooling {
    pub retry_after: i64,
}

/// What the caller knows about the request when picking.
#[derive(Debug, Default, Clone)]
pub struct PickRequest {
    /// Session identity for sticky routing (`metadata.user_id`, else the
    /// terminal key).
    pub sticky_key: Option<String>,
    /// Only accounts in this group (from the user's policy).
    pub group: Option<String>,
    /// Accounts already tried for this request.
    pub exclude: Vec<i64>,
}

#[derive(Clone)]
pub struct AccountPool {
    store: AccountStore,
    oauth: OAuthClient,
    cfg: PoolConfig,
    providers: Arc<Mutex<HashMap<i64, Arc<AccountProvider>>>>,
    sticky: Arc<Mutex<HashMap<String, (i64, Instant)>>>,
    /// Smooth weighted round-robin state: account id → current weight.
    rr: Arc<Mutex<HashMap<i64, i64>>>,
}

impl AccountPool {
    pub fn new(store: AccountStore, oauth: OAuthClient, cfg: PoolConfig) -> Self {
        Self {
            store,
            oauth,
            cfg,
            providers: Default::default(),
            sticky: Default::default(),
            rr: Default::default(),
        }
    }

    pub fn config(&self) -> &PoolConfig {
        &self.cfg
    }

    /// Picks the account for one upstream attempt.
    ///
    /// Eligible: enabled, in the requested group, not excluded. Among those,
    /// healthy accounts (active, not cooling) are chosen by the sticky
    /// binding, then the strategy. With none healthy: if some are only
    /// cooling, fail with [`AllCooling`]; otherwise fall back to the first
    /// eligible account so a needs_reauth account surfaces its real error.
    pub fn pick(&self, req: &PickRequest) -> Result<Arc<AccountProvider>> {
        let now = crate::now_unix();
        let eligible: Vec<Account> = self
            .store
            .list()?
            .into_iter()
            .filter(|a| a.enabled && !req.exclude.contains(&a.id))
            .filter(|a| match &req.group {
                Some(g) => a.account_group == *g,
                None => true,
            })
            .collect();
        let healthy: Vec<&Account> = eligible
            .iter()
            .filter(|a| a.status == STATUS_ACTIVE && a.cooldown_left(now) == 0)
            .collect();
        if healthy.is_empty() {
            if let Some(retry_after) = eligible
                .iter()
                .filter(|a| a.status == STATUS_ACTIVE)
                .map(|a| a.cooldown_left(now))
                .filter(|s| *s > 0)
                .min()
            {
                return Err(AllCooling { retry_after }.into());
            }
            return match eligible.first() {
                Some(a) => Ok(self.provider(a)),
                None => Err(NoAccount.into()),
            };
        }
        let sticky_key = req
            .sticky_key
            .as_ref()
            .filter(|_| self.cfg.sticky)
            .map(|k| format!("{}|{k}", req.group.as_deref().unwrap_or("")));
        if let Some(key) = &sticky_key {
            if let Some(id) = self.sticky_lookup(key) {
                if let Some(a) = healthy.iter().find(|a| a.id == id) {
                    return Ok(self.provider(a));
                }
            }
        }
        let chosen = match self.cfg.strategy {
            Strategy::Priority => healthy[0],
            Strategy::RoundRobin => self.round_robin(&healthy),
            Strategy::LeastUsed => self.least_used(&healthy)?,
        };
        if let Some(key) = sticky_key {
            self.sticky_bind(key, chosen.id);
        }
        Ok(self.provider(chosen))
    }

    fn sticky_lookup(&self, key: &str) -> Option<i64> {
        let mut map = self.sticky.lock().unwrap_or_else(|e| e.into_inner());
        let ttl = Duration::from_secs(self.cfg.sticky_ttl_secs);
        match map.get_mut(key) {
            Some((id, seen)) if seen.elapsed() < ttl => {
                *seen = Instant::now();
                Some(*id)
            }
            Some(_) => {
                map.remove(key);
                None
            }
            None => None,
        }
    }

    fn sticky_bind(&self, key: String, id: i64) {
        let mut map = self.sticky.lock().unwrap_or_else(|e| e.into_inner());
        if map.len() > 10_000 {
            let ttl = Duration::from_secs(self.cfg.sticky_ttl_secs);
            map.retain(|_, (_, seen)| seen.elapsed() < ttl);
        }
        map.insert(key, (id, Instant::now()));
    }

    /// Smooth weighted round-robin (as in nginx): spreads picks in
    /// proportion to `weight` without bursts.
    fn round_robin<'a>(&self, healthy: &[&'a Account]) -> &'a Account {
        let mut cur = self.rr.lock().unwrap_or_else(|e| e.into_inner());
        let total: i64 = healthy.iter().map(|a| a.weight.max(1)).sum();
        let mut best = healthy[0];
        let mut best_w = i64::MIN;
        for a in healthy {
            let w = cur.entry(a.id).or_insert(0);
            *w += a.weight.max(1);
            if *w > best_w {
                best_w = *w;
                best = a;
            }
        }
        *cur.get_mut(&best.id).expect("present") -= total;
        best
    }

    /// Fewest tokens in the last 5 hours; ties keep priority order.
    fn least_used<'a>(&self, healthy: &[&'a Account]) -> Result<&'a Account> {
        let used = self
            .store
            .recent_tokens(crate::now_unix() - LEAST_USED_WINDOW_SECS)?;
        Ok(healthy
            .iter()
            .min_by_key(|a| used.get(&a.id).copied().unwrap_or(0))
            .copied()
            .expect("non-empty"))
    }

    /// Cached per account so the in-memory token survives across requests;
    /// rebuilt when the account's sync settings change.
    fn provider(&self, a: &Account) -> Arc<AccountProvider> {
        let mut map = self.providers.lock().unwrap_or_else(|e| e.into_inner());
        let want: Option<std::path::PathBuf> =
            (a.sync_file && !a.source_path.is_empty()).then(|| a.source_path.clone().into());
        if let Some(p) = map.get(&a.id) {
            if p.sync_path == want {
                return p.clone();
            }
        }
        let p = Arc::new(AccountProvider::new(
            self.store.clone(),
            self.oauth.clone(),
            a,
        ));
        map.insert(a.id, p.clone());
        p
    }

    /// Parks an account for `secs` seconds (capped at a day).
    pub fn cool_down(&self, id: i64, secs: i64, reason: &str) {
        let secs = secs.clamp(1, 86_400);
        tracing::warn!(account = id, "cooling down for {secs}s: {reason}");
        if let Err(e) = self
            .store
            .set_cooldown(id, crate::now_unix() + secs, reason)
        {
            tracing::warn!(account = id, "record cooldown: {e:#}");
        }
    }

    pub fn source(&self) -> String {
        match self.pick(&PickRequest::default()) {
            Ok(p) => p.source(),
            Err(e) => format!("account pool: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::{test_store, Credentials, STATUS_NEEDS_REAUTH};

    fn add(s: &AccountStore, label: &str, priority: i64, weight: i64, group: &str) -> i64 {
        s.create(
            Account {
                label: label.into(),
                enabled: true,
                priority,
                weight,
                account_group: group.into(),
                ..Default::default()
            },
            &Credentials {
                access_token: format!("{label}-tok"),
                ..Default::default()
            },
        )
        .unwrap()
    }

    fn pool(s: &AccountStore, strategy: Strategy, sticky: bool) -> AccountPool {
        AccountPool::new(
            s.clone(),
            OAuthClient::new("http://unused", "cli"),
            PoolConfig {
                strategy,
                sticky,
                ..Default::default()
            },
        )
    }

    fn pick(p: &AccountPool, req: &PickRequest) -> i64 {
        p.pick(req).unwrap().account_id()
    }

    #[test]
    fn priority_skips_disabled_cooling_and_reauth() {
        let s = test_store();
        let low = add(&s, "low", 0, 1, "");
        let high = add(&s, "high", 9, 1, "");
        let p = pool(&s, Strategy::Priority, false);
        let any = PickRequest::default();
        assert_eq!(pick(&p, &any), high);
        p.cool_down(high, 60, "429");
        assert_eq!(pick(&p, &any), low);
        s.update(
            high,
            &crate::accounts::AccountPatch {
                clear_cooldown: true,
                ..Default::default()
            },
        )
        .unwrap();
        s.set_status(high, STATUS_NEEDS_REAUTH, "x").unwrap();
        assert_eq!(pick(&p, &any), low);
        s.set_enabled(low, false).unwrap();
        // Only a needs_reauth account left: still picked so its error surfaces.
        assert_eq!(pick(&p, &any), high);
        s.set_enabled(high, false).unwrap();
        assert!(p.pick(&any).err().unwrap().is::<NoAccount>());
    }

    #[test]
    fn all_cooling_reports_earliest_retry() {
        let s = test_store();
        let a = add(&s, "a", 0, 1, "");
        let b = add(&s, "b", 0, 1, "");
        let p = pool(&s, Strategy::Priority, false);
        p.cool_down(a, 300, "429");
        p.cool_down(b, 30, "429");
        let err = p.pick(&PickRequest::default()).err().unwrap();
        let ac = err.downcast_ref::<AllCooling>().unwrap();
        assert!((29..=30).contains(&ac.retry_after), "{}", ac.retry_after);
    }

    #[test]
    fn exclude_moves_to_next_account() {
        let s = test_store();
        let a = add(&s, "a", 5, 1, "");
        let b = add(&s, "b", 0, 1, "");
        let p = pool(&s, Strategy::Priority, false);
        assert_eq!(
            pick(
                &p,
                &PickRequest {
                    exclude: vec![a],
                    ..Default::default()
                }
            ),
            b
        );
        assert!(p
            .pick(&PickRequest {
                exclude: vec![a, b],
                ..Default::default()
            })
            .is_err());
    }

    #[test]
    fn weighted_round_robin() {
        let s = test_store();
        let a = add(&s, "a", 0, 3, "");
        let b = add(&s, "b", 0, 1, "");
        let p = pool(&s, Strategy::RoundRobin, false);
        let picks: Vec<i64> = (0..8).map(|_| pick(&p, &PickRequest::default())).collect();
        assert_eq!(picks.iter().filter(|&&x| x == a).count(), 6);
        assert_eq!(picks.iter().filter(|&&x| x == b).count(), 2);
        // Smooth: never more than three of `a` in a row.
        assert!(!picks.windows(4).any(|w| w.iter().all(|&x| x == a)));
    }

    #[test]
    fn least_used_prefers_idle_account() {
        let s = test_store();
        let busy = add(&s, "busy", 9, 1, "");
        let idle = add(&s, "idle", 0, 1, "");
        s.store_for_tests()
            .conn()
            .execute(
                "INSERT INTO usage_events(ts, user_id, key_id, account_id, model, input_tokens, output_tokens, latency_ms, status)
                 VALUES(?1, 1, 1, ?2, 'm', 500, 500, 1, 200)",
                rusqlite::params![crate::now_unix(), busy],
            )
            .unwrap();
        let p = pool(&s, Strategy::LeastUsed, false);
        assert_eq!(pick(&p, &PickRequest::default()), idle);
    }

    #[test]
    fn sticky_sessions_stay_until_unhealthy() {
        let s = test_store();
        let a = add(&s, "a", 0, 1, "");
        let b = add(&s, "b", 0, 1, "");
        let p = pool(&s, Strategy::RoundRobin, true);
        let sess = |k: &str| PickRequest {
            sticky_key: Some(k.into()),
            ..Default::default()
        };
        let first = pick(&p, &sess("s1"));
        for _ in 0..5 {
            assert_eq!(pick(&p, &sess("s1")), first);
        }
        // A different session gets the other account under round-robin.
        assert_ne!(pick(&p, &sess("s2")), first);
        // The bound account cools down: the session moves and stays moved.
        p.cool_down(first, 60, "429");
        let moved = pick(&p, &sess("s1"));
        assert_ne!(moved, first);
        assert!([a, b].contains(&moved));
        s.update(
            first,
            &crate::accounts::AccountPatch {
                clear_cooldown: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(pick(&p, &sess("s1")), moved);
    }

    #[test]
    fn groups_restrict_accounts() {
        let s = test_store();
        let team = add(&s, "team", 0, 1, "team");
        let other = add(&s, "other", 9, 1, "");
        let p = pool(&s, Strategy::Priority, false);
        assert_eq!(pick(&p, &PickRequest::default()), other);
        let req = PickRequest {
            group: Some("team".into()),
            ..Default::default()
        };
        assert_eq!(pick(&p, &req), team);
        let req = PickRequest {
            group: Some("nobody".into()),
            ..Default::default()
        };
        assert!(p.pick(&req).err().unwrap().is::<NoAccount>());
    }
}
