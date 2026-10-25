//! Usage analytics for the stats page: time series (hour / day / week,
//! optionally split by model, account or terminal), rollups, a weekday ×
//! hour heatmap, and estimated cost from an editable price table.
//!
//! Buckets are computed in the viewer's time zone (`tz_offset_secs` east of
//! UTC). Costs are what the same traffic would cost on the pay-as-you-go
//! API — Pro/Max subscriptions are not billed per token.

use std::collections::BTreeMap;

use anyhow::{bail, Context, Result};
use regex::Regex;
use rusqlite::params_from_iter;
use rusqlite::types::Value as Sql;
use serde::{Deserialize, Serialize};

use crate::store::Store;

const SETTING_PRICES: &str = "model_prices";

/// USD per million tokens for models matching `pattern` (an exact name,
/// a case-insensitive substring, or `re:` + regex). First match wins.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Price {
    pub pattern: String,
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

impl Price {
    fn matches(&self, model: &str) -> bool {
        match self.pattern.strip_prefix("re:") {
            Some(re) => Regex::new(re).is_ok_and(|r| r.is_match(model)),
            None => model
                .to_ascii_lowercase()
                .contains(&self.pattern.to_ascii_lowercase()),
        }
    }

    fn cost(&self, t: &Tokens) -> f64 {
        (t.input_tokens as f64 * self.input
            + t.output_tokens as f64 * self.output
            + t.cache_read_tokens as f64 * self.cache_read
            + t.cache_creation_tokens as f64 * self.cache_write)
            / 1_000_000.0
    }
}

fn price(pattern: &str, input: f64, output: f64) -> Price {
    Price {
        pattern: pattern.into(),
        input,
        output,
        cache_read: input / 10.0,
        cache_write: input * 1.25,
    }
}

/// Published list prices for the Claude families at the time of writing;
/// edit them on the stats page when they change.
pub fn default_prices() -> Vec<Price> {
    vec![
        price("re:opus-4(-1)?(-\\d{8})?$", 15.0, 75.0),
        price("re:3-opus", 15.0, 75.0),
        price("opus", 5.0, 25.0),
        price("sonnet", 3.0, 15.0),
        price("re:3-5-haiku", 0.8, 4.0),
        price("re:3-haiku", 0.25, 1.25),
        price("haiku", 1.0, 5.0),
    ]
}

#[derive(Clone)]
pub struct Analytics {
    store: Store,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Granularity {
    Hour,
    #[default]
    Day,
    Week,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum GroupBy {
    #[default]
    None,
    Model,
    Account,
    User,
}

/// Which events to look at.
#[derive(Debug, Clone, Default)]
pub struct Scope {
    pub since: i64,
    pub user_id: Option<i64>,
    pub tz_offset_secs: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Tokens {
    pub requests: i64,
    pub errors: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_creation_tokens: i64,
    pub cost_usd: f64,
}

impl Tokens {
    fn add(&mut self, o: &Tokens) {
        self.requests += o.requests;
        self.errors += o.errors;
        self.input_tokens += o.input_tokens;
        self.output_tokens += o.output_tokens;
        self.cache_read_tokens += o.cache_read_tokens;
        self.cache_creation_tokens += o.cache_creation_tokens;
        self.cost_usd += o.cost_usd;
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Point {
    /// `2026-10-25 14:00`, `2026-10-25`, or the Monday starting the week.
    pub bucket: String,
    /// Model name, account id, user id, or empty when not grouped.
    pub key: String,
    #[serde(flatten)]
    pub tokens: Tokens,
}

#[derive(Debug, Clone, Serialize)]
pub struct Row {
    pub key: String,
    #[serde(flatten)]
    pub tokens: Tokens,
}

#[derive(Debug, Clone, Serialize)]
pub struct HeatCell {
    /// 0 = Monday … 6 = Sunday.
    pub weekday: i64,
    pub hour: i64,
    pub requests: i64,
    pub tokens: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Overview {
    pub series: Vec<Point>,
    pub by_model: Vec<Row>,
    pub by_account: Vec<Row>,
    pub by_user: Vec<Row>,
    pub totals: Tokens,
    pub heatmap: Vec<HeatCell>,
}

/// One aggregated row: (bucket, model, account, user) → tokens.
struct Raw {
    bucket: String,
    model: String,
    account: String,
    user: String,
    tokens: Tokens,
}

impl Analytics {
    pub fn new(store: Store) -> Self {
        Self { store }
    }

    pub fn prices(&self) -> Result<Vec<Price>> {
        match self.store.get_setting(SETTING_PRICES)? {
            Some(v) if !v.is_empty() => Ok(serde_json::from_str(&v)?),
            _ => Ok(default_prices()),
        }
    }

    /// Replaces the price table; an empty list restores the defaults.
    pub fn set_prices(&self, prices: &[Price]) -> Result<()> {
        for p in prices {
            if p.pattern.trim().is_empty() {
                bail!("each price needs a pattern");
            }
            if let Some(re) = p.pattern.strip_prefix("re:") {
                Regex::new(re).with_context(|| format!("invalid regex {:?}", p.pattern))?;
            }
            if [p.input, p.output, p.cache_read, p.cache_write]
                .iter()
                .any(|v| !v.is_finite() || *v < 0.0)
            {
                bail!("prices must be non-negative numbers");
            }
        }
        let v = if prices.is_empty() {
            String::new()
        } else {
            serde_json::to_string(prices)?
        };
        self.store.set_setting(SETTING_PRICES, &v)
    }

    pub fn overview(&self, scope: &Scope, gran: Granularity, group: GroupBy) -> Result<Overview> {
        let prices = self.prices()?;
        let raw = self.raw(scope, gran)?;
        let mut series: BTreeMap<(String, String), Tokens> = BTreeMap::new();
        let mut by_model: BTreeMap<String, Tokens> = BTreeMap::new();
        let mut by_account: BTreeMap<String, Tokens> = BTreeMap::new();
        let mut by_user: BTreeMap<String, Tokens> = BTreeMap::new();
        let mut totals = Tokens::default();
        for mut r in raw {
            r.tokens.cost_usd = prices
                .iter()
                .find(|p| p.matches(&r.model))
                .map_or(0.0, |p| p.cost(&r.tokens));
            let key = match group {
                GroupBy::None => String::new(),
                GroupBy::Model => r.model.clone(),
                GroupBy::Account => r.account.clone(),
                GroupBy::User => r.user.clone(),
            };
            series.entry((r.bucket, key)).or_default().add(&r.tokens);
            by_model.entry(r.model).or_default().add(&r.tokens);
            by_account.entry(r.account).or_default().add(&r.tokens);
            by_user.entry(r.user).or_default().add(&r.tokens);
            totals.add(&r.tokens);
        }
        let rows = |m: BTreeMap<String, Tokens>| {
            let mut v: Vec<Row> = m
                .into_iter()
                .map(|(key, tokens)| Row { key, tokens })
                .collect();
            v.sort_by_key(|r| std::cmp::Reverse(r.tokens.input_tokens + r.tokens.output_tokens));
            v
        };
        Ok(Overview {
            series: series
                .into_iter()
                .map(|((bucket, key), tokens)| Point {
                    bucket,
                    key,
                    tokens,
                })
                .collect(),
            by_model: rows(by_model),
            by_account: rows(by_account),
            by_user: rows(by_user),
            totals,
            heatmap: self.heatmap(scope)?,
        })
    }

    fn filter(scope: &Scope) -> (String, Vec<Sql>) {
        let mut sql = String::from("ts >= ?1");
        let mut args = vec![
            Sql::Integer(scope.since),
            Sql::Integer(scope.tz_offset_secs),
        ];
        if let Some(u) = scope.user_id {
            sql.push_str(" AND user_id = ?3");
            args.push(Sql::Integer(u));
        }
        (sql, args)
    }

    fn raw(&self, scope: &Scope, gran: Granularity) -> Result<Vec<Raw>> {
        let bucket = match gran {
            Granularity::Hour => "strftime('%Y-%m-%d %H:00', ts + ?2, 'unixepoch')",
            Granularity::Day => "date(ts + ?2, 'unixepoch')",
            // The Monday on or before the day.
            Granularity::Week => "date(ts + ?2, 'unixepoch', '-6 days', 'weekday 1')",
        };
        let (filter, args) = Self::filter(scope);
        let conn = self.store.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {bucket} AS b, model, COALESCE(account_id, 0), user_id, COUNT(*),
                    COALESCE(SUM(status >= 400), 0),
                    COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0),
                    COALESCE(SUM(cache_read_tokens), 0), COALESCE(SUM(cache_creation_tokens), 0)
             FROM usage_events WHERE {filter}
             GROUP BY b, model, account_id, user_id ORDER BY b"
        ))?;
        let rows = stmt.query_map(params_from_iter(args), |r| {
            Ok(Raw {
                bucket: r.get(0)?,
                model: r.get(1)?,
                account: r.get::<_, i64>(2)?.to_string(),
                user: r.get::<_, i64>(3)?.to_string(),
                tokens: Tokens {
                    requests: r.get(4)?,
                    errors: r.get(5)?,
                    input_tokens: r.get(6)?,
                    output_tokens: r.get(7)?,
                    cache_read_tokens: r.get(8)?,
                    cache_creation_tokens: r.get(9)?,
                    cost_usd: 0.0,
                },
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    fn heatmap(&self, scope: &Scope) -> Result<Vec<HeatCell>> {
        let (filter, args) = Self::filter(scope);
        let conn = self.store.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT (CAST(strftime('%w', ts + ?2, 'unixepoch') AS INTEGER) + 6) % 7 AS wd,
                    CAST(strftime('%H', ts + ?2, 'unixepoch') AS INTEGER) AS h,
                    COUNT(*), COALESCE(SUM(input_tokens + output_tokens), 0)
             FROM usage_events WHERE {filter} GROUP BY wd, h ORDER BY wd, h"
        ))?;
        let rows = stmt.query_map(params_from_iter(args), |r| {
            Ok(HeatCell {
                weekday: r.get(0)?,
                hour: r.get(1)?,
                requests: r.get(2)?,
                tokens: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Requests and tokens per user since `since` (e.g. today's counts on
    /// the terminals page).
    pub fn per_user_since(&self, since: i64) -> Result<BTreeMap<i64, (i64, i64)>> {
        let conn = self.store.conn();
        let mut stmt = conn.prepare(
            "SELECT user_id, COUNT(*), COALESCE(SUM(input_tokens + output_tokens), 0)
             FROM usage_events WHERE ts >= ?1 GROUP BY user_id",
        )?;
        let rows = stmt.query_map([since], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::Keys;
    use crate::stats::{Event, Recorder};

    #[test]
    fn series_rollups_heatmap_and_cost() {
        let store = Store::open_in_memory().unwrap();
        let keys = Keys::new(store.clone());
        let alice = keys.create_user("alice").unwrap();
        let bob = keys.create_user("bob").unwrap();
        let rec = Recorder::new(store.clone());
        // 2026-10-19 (a Monday) 23:30 UTC, and two events the next day.
        let mon = 1_792_452_600;
        for (ts, user, model, input, output, status) in [
            (mon, alice, "claude-sonnet-4-5", 1_000_000, 0, 200),
            (mon + 3600, alice, "claude-opus-4-1", 0, 1_000_000, 200),
            (mon + 7200, bob, "claude-haiku-4-5", 10, 10, 529),
        ] {
            rec.record(Event {
                ts,
                user_id: user,
                key_id: 1,
                account_id: 1,
                model: model.into(),
                input_tokens: input,
                output_tokens: output,
                status,
                ..Default::default()
            });
        }
        rec.flush();
        let a = Analytics::new(store);
        let utc = Scope::default();
        let o = a.overview(&utc, Granularity::Day, GroupBy::Model).unwrap();
        let buckets: Vec<_> = o
            .series
            .iter()
            .map(|p| (p.bucket.as_str(), p.key.as_str()))
            .collect();
        assert_eq!(
            buckets,
            [
                ("2026-10-19", "claude-sonnet-4-5"),
                ("2026-10-20", "claude-haiku-4-5"),
                ("2026-10-20", "claude-opus-4-1")
            ]
        );
        // Sonnet $3/M in, opus-4-1 $75/M out.
        assert!(
            (o.totals.cost_usd - 78.0).abs() < 0.01,
            "{}",
            o.totals.cost_usd
        );
        assert_eq!((o.totals.requests, o.totals.errors), (3, 1));
        assert_eq!(o.by_user.len(), 2);

        // UTC+8 moves the first event to Tuesday 07:30.
        let cst = Scope {
            tz_offset_secs: 8 * 3600,
            ..Default::default()
        };
        let o = a.overview(&cst, Granularity::Week, GroupBy::None).unwrap();
        assert_eq!(o.series.len(), 1);
        assert_eq!(o.series[0].bucket, "2026-10-19");
        assert!(o.heatmap.iter().any(|c| c.weekday == 1 && c.hour == 7));

        let only_bob = Scope {
            user_id: Some(bob),
            ..Default::default()
        };
        let o = a
            .overview(&only_bob, Granularity::Hour, GroupBy::None)
            .unwrap();
        assert_eq!(o.totals.requests, 1);
        assert_eq!(o.series[0].bucket, "2026-10-20 01:00");
    }

    #[test]
    fn prices_validate_and_reset() {
        let a = Analytics::new(Store::open_in_memory().unwrap());
        assert_eq!(a.prices().unwrap(), default_prices());
        assert!(a.set_prices(&[price("re:(", 1.0, 1.0)]).is_err());
        a.set_prices(&[price("claude", 1.0, 2.0)]).unwrap();
        assert_eq!(a.prices().unwrap().len(), 1);
        a.set_prices(&[]).unwrap();
        assert_eq!(a.prices().unwrap(), default_prices());
        let p = default_prices();
        let opus = |m: &str| p.iter().find(|x| x.matches(m)).unwrap().input;
        assert_eq!(opus("claude-opus-4-1-20250805"), 15.0);
        assert_eq!(opus("claude-opus-4-20250514"), 15.0);
        assert_eq!(opus("claude-opus-4-5"), 5.0);
        assert_eq!(opus("claude-3-5-haiku-latest"), 0.8);
    }
}
