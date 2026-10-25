//! Client-IP security for the proxy surface (`/v1/*`, `/register`): CIDR
//! deny / allow rules with optional expiry, and an access log (written on
//! its own thread, pruned by age) behind the IP statistics.
//!
//! A deny match always blocks. While any allow rule is active, only IPs
//! matching one get through (allowlist mode). The admin API and the local
//! desktop bridge are never filtered, so a bad rule can't lock the admin
//! out.

use std::net::IpAddr;
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::{Arc, RwLock};

use anyhow::{bail, Context, Result};
use rusqlite::{params, params_from_iter, types::Value as Sql};
use serde::{Deserialize, Serialize};

use crate::store::Store;

const QUEUE: usize = 1024;

/// An IPv4 / IPv6 network; a bare address is a /32 or /128.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Net {
    addr: IpAddr,
    prefix: u8,
}

impl std::str::FromStr for Net {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        let s = s.trim();
        let (a, p) = match s.split_once('/') {
            Some((a, p)) => (a, Some(p)),
            None => (s, None),
        };
        let addr: IpAddr = a
            .parse::<IpAddr>()
            .with_context(|| format!("{a:?} is not an IP address"))?
            .to_canonical();
        let max = if addr.is_ipv4() { 32 } else { 128 };
        let prefix = match p {
            Some(p) => p
                .parse::<u8>()
                .ok()
                .filter(|p| *p <= max)
                .with_context(|| format!("prefix /{p} out of range for {a}"))?,
            None => max,
        };
        Ok(Net { addr, prefix })
    }
}

impl Net {
    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.addr, ip.to_canonical()) {
            (IpAddr::V4(n), IpAddr::V4(i)) => {
                let mask = u32::MAX
                    .checked_shl(32 - u32::from(self.prefix))
                    .unwrap_or(0);
                u32::from(n) & mask == u32::from(i) & mask
            }
            (IpAddr::V6(n), IpAddr::V6(i)) => {
                let mask = u128::MAX
                    .checked_shl(128 - u32::from(self.prefix))
                    .unwrap_or(0);
                u128::from(n) & mask == u128::from(i) & mask
            }
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleKind {
    Allow,
    Deny,
}

impl RuleKind {
    fn as_str(self) -> &'static str {
        match self {
            RuleKind::Allow => "allow",
            RuleKind::Deny => "deny",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Rule {
    pub id: i64,
    pub kind: RuleKind,
    pub cidr: String,
    pub note: String,
    pub expires_at: Option<i64>,
    pub created_at: i64,
    pub hits: i64,
    #[serde(skip)]
    net: Option<Net>,
}

impl Rule {
    fn active(&self, now: i64) -> bool {
        self.expires_at.is_none_or(|t| t > now)
    }
}

/// Why a request was turned away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Block {
    /// Matched this deny rule.
    Denied(i64),
    /// Allowlist mode and no allow rule matched.
    NotAllowed,
}

/// One logged request.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Access {
    pub ts: i64,
    pub ip: String,
    pub method: String,
    pub path: String,
    pub status: u16,
    pub blocked: bool,
    pub rule_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AccessRow {
    pub id: i64,
    #[serde(flatten)]
    pub access: Access,
}

#[derive(Debug, Clone, Default)]
pub struct LogFilter {
    pub ip: String,
    pub blocked: Option<bool>,
    pub limit: i64,
    pub offset: i64,
}

/// Per-IP totals over a period, with the terminals seen from it.
#[derive(Debug, Clone, Serialize)]
pub struct IpStat {
    pub ip: String,
    pub requests: i64,
    pub blocked: i64,
    pub errors: i64,
    pub first_seen: i64,
    pub last_seen: i64,
    /// Labels of terminals that used this IP (from the IP tracker).
    pub terminals: Vec<String>,
}

enum Msg {
    Access(Access),
    Flush(SyncSender<()>),
}

#[derive(Clone)]
pub struct Security {
    store: Store,
    rules: Arc<RwLock<Vec<Rule>>>,
    tx: SyncSender<Msg>,
}

impl Security {
    pub fn new(store: Store) -> Result<Self> {
        let (tx, rx) = sync_channel(QUEUE);
        let writer = store.clone();
        std::thread::Builder::new()
            .name("ccm-access-log".into())
            .spawn(move || run(writer, rx))?;
        let s = Self {
            store,
            rules: Default::default(),
            tx,
        };
        s.reload()?;
        Ok(s)
    }

    fn reload(&self) -> Result<()> {
        let rules = self.read_rules()?;
        *self.rules.write().unwrap_or_else(|e| e.into_inner()) = rules;
        Ok(())
    }

    fn read_rules(&self) -> Result<Vec<Rule>> {
        let conn = self.store.conn();
        let mut stmt = conn.prepare(
            "SELECT id, kind, cidr, note, expires_at, created_at, hits FROM ip_rules ORDER BY id",
        )?;
        let rows = stmt.query_map([], |r| {
            let kind: String = r.get(1)?;
            let cidr: String = r.get(2)?;
            Ok(Rule {
                id: r.get(0)?,
                kind: if kind == "allow" {
                    RuleKind::Allow
                } else {
                    RuleKind::Deny
                },
                net: cidr.parse().ok(),
                cidr,
                note: r.get(3)?,
                expires_at: r.get(4)?,
                created_at: r.get(5)?,
                hits: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Rules with their current hit counts.
    pub fn rules(&self) -> Result<Vec<Rule>> {
        self.flush();
        self.read_rules()
    }

    pub fn add_rule(
        &self,
        kind: RuleKind,
        cidr: &str,
        note: &str,
        expires_at: Option<i64>,
    ) -> Result<i64> {
        let net: Net = cidr.parse()?;
        let canonical = if (net.addr.is_ipv4() && net.prefix == 32) || net.prefix == 128 {
            net.addr.to_string()
        } else {
            format!("{}/{}", net.addr, net.prefix)
        };
        let id = {
            let conn = self.store.conn();
            conn.execute(
                "INSERT INTO ip_rules(kind, cidr, note, expires_at, created_at) VALUES(?1, ?2, ?3, ?4, ?5)",
                params![kind.as_str(), canonical, note.trim(), expires_at, crate::now_unix()],
            )?;
            conn.last_insert_rowid()
        };
        self.reload()?;
        Ok(id)
    }

    pub fn delete_rule(&self, id: i64) -> Result<()> {
        let n = self
            .store
            .conn()
            .execute("DELETE FROM ip_rules WHERE id = ?1", [id])?;
        if n == 0 {
            bail!("rule {id} not found");
        }
        self.reload()
    }

    /// Whether `ip` may use the proxy. Unparseable or empty IPs (in-process
    /// calls) are allowed.
    pub fn check(&self, ip: &str) -> Result<(), Block> {
        let Ok(addr) = ip.parse::<IpAddr>() else {
            return Ok(());
        };
        let now = crate::now_unix();
        let rules = self.rules.read().unwrap_or_else(|e| e.into_inner());
        let matching = |kind: RuleKind| {
            rules
                .iter()
                .filter(move |r| r.kind == kind && r.active(now))
                .filter(move |r| r.net.is_some_and(|n| n.contains(addr)))
        };
        if let Some(r) = matching(RuleKind::Deny).next() {
            return Err(Block::Denied(r.id));
        }
        let allowlist = rules
            .iter()
            .any(|r| r.kind == RuleKind::Allow && r.active(now));
        if allowlist && matching(RuleKind::Allow).next().is_none() {
            return Err(Block::NotAllowed);
        }
        Ok(())
    }

    /// Queues an access-log entry; dropped (with a warning) if the writer
    /// is backed up.
    pub fn log(&self, a: Access) {
        if self.tx.try_send(Msg::Access(a)).is_err() {
            tracing::warn!("access log: queue full, entry dropped");
        }
    }

    /// Blocks until queued entries are written.
    pub fn flush(&self) {
        let (done_tx, done_rx) = sync_channel(1);
        if self.tx.send(Msg::Flush(done_tx)).is_ok() {
            let _ = done_rx.recv();
        }
    }

    /// Newest first, plus the total under the filter.
    pub fn access_log(&self, f: &LogFilter) -> Result<(Vec<AccessRow>, i64)> {
        self.flush();
        let mut wheres = vec!["1 = 1".to_string()];
        let mut args: Vec<Sql> = vec![];
        if !f.ip.trim().is_empty() {
            wheres.push("ip LIKE ?".into());
            args.push(Sql::Text(format!("%{}%", f.ip.trim())));
        }
        if let Some(b) = f.blocked {
            wheres.push("blocked = ?".into());
            args.push(Sql::Integer(b as i64));
        }
        let w = wheres.join(" AND ");
        let conn = self.store.conn();
        let total: i64 = conn.query_row(
            &format!("SELECT COUNT(*) FROM ip_access WHERE {w}"),
            params_from_iter(args.iter()),
            |r| r.get(0),
        )?;
        let limit = if f.limit <= 0 { 50 } else { f.limit.min(500) };
        let mut list_args = args;
        list_args.push(Sql::Integer(limit));
        list_args.push(Sql::Integer(f.offset.max(0)));
        let mut stmt = conn.prepare(&format!(
            "SELECT id, ts, ip, method, path, status, blocked, rule_id FROM ip_access
             WHERE {w} ORDER BY ts DESC, id DESC LIMIT ? OFFSET ?"
        ))?;
        let rows = stmt.query_map(params_from_iter(list_args), |r| {
            Ok(AccessRow {
                id: r.get(0)?,
                access: Access {
                    ts: r.get(1)?,
                    ip: r.get(2)?,
                    method: r.get(3)?,
                    path: r.get(4)?,
                    status: r.get(5)?,
                    blocked: r.get(6)?,
                    rule_id: r.get(7)?,
                },
            })
        })?;
        Ok((rows.collect::<rusqlite::Result<_>>()?, total))
    }

    /// Per-IP totals since `since`, busiest first.
    pub fn ip_stats(&self, since: i64, limit: i64) -> Result<Vec<IpStat>> {
        self.flush();
        let conn = self.store.conn();
        let mut stmt = conn.prepare(
            "SELECT ip, COUNT(*), COALESCE(SUM(blocked), 0), COALESCE(SUM(status >= 400 AND blocked = 0), 0),
                    MIN(ts), MAX(ts)
             FROM ip_access WHERE ts >= ?1 GROUP BY ip ORDER BY COUNT(*) DESC LIMIT ?2",
        )?;
        let mut stats: Vec<IpStat> = stmt
            .query_map(params![since, limit.clamp(1, 1000)], |r| {
                Ok(IpStat {
                    ip: r.get(0)?,
                    requests: r.get(1)?,
                    blocked: r.get(2)?,
                    errors: r.get(3)?,
                    first_seen: r.get(4)?,
                    last_seen: r.get(5)?,
                    terminals: vec![],
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        let mut users = conn.prepare(
            "SELECT u.label FROM user_ips i JOIN users u ON u.id = i.user_id
             WHERE i.ip = ?1 ORDER BY i.last_seen DESC",
        )?;
        for s in &mut stats {
            s.terminals = users
                .query_map([&s.ip], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
        }
        Ok(stats)
    }

    /// Deletes access-log entries older than `days`.
    pub fn prune(&self, days: i64) -> Result<usize> {
        let cutoff = crate::now_unix() - days.max(1) * 86_400;
        Ok(self
            .store
            .conn()
            .execute("DELETE FROM ip_access WHERE ts < ?1", [cutoff])?)
    }
}

fn run(store: Store, rx: Receiver<Msg>) {
    while let Ok(msg) = rx.recv() {
        match msg {
            Msg::Access(a) => {
                let conn = store.conn();
                let r = conn.execute(
                    "INSERT INTO ip_access(ts, ip, method, path, status, blocked, rule_id)
                     VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![a.ts, a.ip, a.method, a.path, a.status, a.blocked, a.rule_id],
                );
                if let Some(id) = a.rule_id {
                    let _ = conn.execute("UPDATE ip_rules SET hits = hits + 1 WHERE id = ?1", [id]);
                }
                if let Err(e) = r {
                    tracing::warn!("access log write: {e}");
                }
            }
            Msg::Flush(done) => {
                let _ = done.send(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cidr_parse_and_match() {
        let n: Net = "10.1.0.0/16".parse().unwrap();
        assert!(n.contains("10.1.200.3".parse().unwrap()));
        assert!(!n.contains("10.2.0.1".parse().unwrap()));
        assert!(n.contains("::ffff:10.1.0.9".parse().unwrap()), "v4-mapped");
        let one: Net = "192.168.1.5".parse().unwrap();
        assert!(one.contains("192.168.1.5".parse().unwrap()));
        assert!(!one.contains("192.168.1.6".parse().unwrap()));
        let all: Net = "0.0.0.0/0".parse().unwrap();
        assert!(all.contains("8.8.8.8".parse().unwrap()));
        let v6: Net = "fd00::/8".parse().unwrap();
        assert!(v6.contains("fd12::1".parse().unwrap()));
        assert!(!v6.contains("10.0.0.1".parse().unwrap()));
        assert!("10.0.0.0/33".parse::<Net>().is_err());
        assert!("nope".parse::<Net>().is_err());
    }

    #[test]
    fn deny_wins_and_allowlist_mode() {
        let s = Security::new(Store::open_in_memory().unwrap()).unwrap();
        assert_eq!(s.check("1.2.3.4"), Ok(()));
        assert_eq!(s.check(""), Ok(()), "in-process calls pass");
        let deny = s
            .add_rule(RuleKind::Deny, "1.2.3.0/24", "bad", None)
            .unwrap();
        assert_eq!(s.check("1.2.3.4"), Err(Block::Denied(deny)));
        s.add_rule(RuleKind::Allow, "10.0.0.0/8", "lan", None)
            .unwrap();
        assert_eq!(s.check("10.9.9.9"), Ok(()));
        assert_eq!(s.check("8.8.8.8"), Err(Block::NotAllowed));
        // An expired rule doesn't count, not even towards allowlist mode.
        s.add_rule(RuleKind::Deny, "10.9.9.9", "", Some(crate::now_unix() - 1))
            .unwrap();
        assert_eq!(s.check("10.9.9.9"), Ok(()));
        s.delete_rule(deny).unwrap();
        let rules = s.rules().unwrap();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].cidr, "10.0.0.0/8");
        assert!(s.add_rule(RuleKind::Deny, "300.1.1.1", "", None).is_err());
    }

    #[test]
    fn access_log_stats_and_prune() {
        let s = Security::new(Store::open_in_memory().unwrap()).unwrap();
        let now = crate::now_unix();
        let rule = s.add_rule(RuleKind::Deny, "6.6.6.6", "", None).unwrap();
        for (ts, ip, status, blocked) in [
            (now, "1.1.1.1", 200, false),
            (now, "1.1.1.1", 429, false),
            (now, "6.6.6.6", 403, true),
            (now - 30 * 86_400, "2.2.2.2", 200, false),
        ] {
            s.log(Access {
                ts,
                ip: ip.into(),
                method: "POST".into(),
                path: "/v1/messages".into(),
                status,
                blocked,
                rule_id: blocked.then_some(rule),
            });
        }
        let (rows, total) = s
            .access_log(&LogFilter {
                blocked: Some(true),
                ..Default::default()
            })
            .unwrap();
        assert_eq!((rows.len(), total), (1, 1));
        assert_eq!(s.rules().unwrap()[0].hits, 1);
        let stats = s.ip_stats(now - 86_400, 10).unwrap();
        assert_eq!(stats[0].ip, "1.1.1.1");
        assert_eq!((stats[0].requests, stats[0].errors), (2, 1));
        assert_eq!(s.prune(7).unwrap(), 1);
        assert_eq!(s.access_log(&LogFilter::default()).unwrap().1, 3);
    }
}
