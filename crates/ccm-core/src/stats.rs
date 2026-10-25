//! Usage events: recorded asynchronously on a writer thread, queried for the
//! admin API (daily / per-user / per-model rollups and a filtered search).

use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};

use anyhow::Result;
use rusqlite::{params, params_from_iter, types::Value};
use serde::Serialize;

use crate::store::Store;

const QUEUE: usize = 256;

#[derive(Debug, Clone, Default)]
pub struct Event {
    pub ts: i64,
    pub user_id: i64,
    pub key_id: i64,
    /// Upstream account that served the request; 0 when none was picked.
    pub account_id: i64,
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_creation_tokens: i64,
    pub latency_ms: i64,
    pub status: u16,
    pub error: String,
}

enum Msg {
    Event(Event),
    Flush(SyncSender<()>),
}

/// Handlers push events without blocking; a background thread writes them.
#[derive(Clone)]
pub struct Recorder {
    store: Store,
    tx: SyncSender<Msg>,
}

impl Recorder {
    pub fn new(store: Store) -> Self {
        let (tx, rx) = sync_channel(QUEUE);
        let writer = store.clone();
        std::thread::Builder::new()
            .name("ccm-stats".into())
            .spawn(move || run(writer, rx))
            .expect("spawn stats writer");
        Self { store, tx }
    }

    pub fn record(&self, e: Event) {
        match self.tx.try_send(Msg::Event(e)) {
            Ok(()) => {}
            Err(TrySendError::Full(Msg::Event(e))) => {
                tracing::warn!(user = e.user_id, model = %e.model, "stats: drop event (queue full)")
            }
            Err(_) => tracing::warn!("stats: writer stopped"),
        }
    }

    /// Blocks until every event recorded so far is written.
    pub fn flush(&self) {
        let (done_tx, done_rx) = sync_channel(1);
        if self.tx.send(Msg::Flush(done_tx)).is_ok() {
            let _ = done_rx.recv();
        }
    }

    pub fn by_day(&self, days: i64) -> Result<Vec<DailySummary>> {
        let conn = self.store.conn();
        let mut stmt = conn.prepare(
            "SELECT date(ts, 'unixepoch') AS d, COUNT(*),
                    COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0)
             FROM usage_events WHERE ts >= ?1 GROUP BY d ORDER BY d",
        )?;
        let rows = stmt.query_map([since(days)], |r| {
            Ok(DailySummary {
                date: r.get(0)?,
                requests: r.get(1)?,
                input_tokens: r.get(2)?,
                output_tokens: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn by_user(&self, days: i64) -> Result<Vec<UserSummary>> {
        let conn = self.store.conn();
        let mut stmt = conn.prepare(
            "SELECT u.id, u.label, COUNT(*),
                    COALESCE(SUM(e.input_tokens), 0), COALESCE(SUM(e.output_tokens), 0)
             FROM usage_events e JOIN users u ON u.id = e.user_id
             WHERE e.ts >= ?1 GROUP BY u.id, u.label ORDER BY u.id",
        )?;
        let rows = stmt.query_map([since(days)], |r| {
            Ok(UserSummary {
                user_id: r.get(0)?,
                label: r.get(1)?,
                requests: r.get(2)?,
                input_tokens: r.get(3)?,
                output_tokens: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn by_model(&self, days: i64) -> Result<Vec<ModelSummary>> {
        self.grouped(days, "model").map(|rows| {
            rows.into_iter()
                .map(
                    |(model, requests, input_tokens, output_tokens)| ModelSummary {
                        model,
                        requests,
                        input_tokens,
                        output_tokens,
                    },
                )
                .collect()
        })
    }

    pub fn by_account(&self, days: i64) -> Result<Vec<AccountSummary>> {
        let conn = self.store.conn();
        let mut stmt = conn.prepare(
            "SELECT e.account_id, COALESCE(a.label, '(deleted)'), COUNT(*),
                    COALESCE(SUM(e.input_tokens), 0), COALESCE(SUM(e.output_tokens), 0)
             FROM usage_events e LEFT JOIN accounts a ON a.id = e.account_id
             WHERE e.ts >= ?1 AND e.account_id IS NOT NULL
             GROUP BY e.account_id ORDER BY e.account_id",
        )?;
        let rows = stmt.query_map([since(days)], |r| {
            Ok(AccountSummary {
                account_id: r.get(0)?,
                label: r.get(1)?,
                requests: r.get(2)?,
                input_tokens: r.get(3)?,
                output_tokens: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    fn grouped(&self, days: i64, col: &str) -> Result<Vec<(String, i64, i64, i64)>> {
        let conn = self.store.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {col}, COUNT(*), COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0)
             FROM usage_events WHERE ts >= ?1 GROUP BY {col} ORDER BY {col}"
        ))?;
        let rows = stmt.query_map([since(days)], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Matching rows (one page) plus the total count under the same filter.
    pub fn search(&self, f: &SearchFilter) -> Result<(Vec<EventRow>, i64)> {
        let limit = match f.limit {
            l if l <= 0 => 50,
            l => l.min(500),
        };
        let mut wheres = vec!["1=1".to_string()];
        let mut args: Vec<Value> = vec![];
        if f.user_id > 0 {
            wheres.push("e.user_id = ?".into());
            args.push(f.user_id.into());
        }
        if f.account_id > 0 {
            wheres.push("e.account_id = ?".into());
            args.push(f.account_id.into());
        }
        if !f.model.trim().is_empty() {
            wheres.push("e.model LIKE ?".into());
            args.push(format!("%{}%", f.model.trim()).into());
        }
        if let Some(t) = f.since {
            wheres.push("e.ts >= ?".into());
            args.push(t.into());
        }
        if let Some(t) = f.until {
            wheres.push("e.ts <= ?".into());
            args.push(t.into());
        }
        if f.min_tokens > 0 {
            wheres.push("(e.input_tokens + e.output_tokens) >= ?".into());
            args.push(f.min_tokens.into());
        }
        if f.only_error {
            wheres.push("(e.error IS NOT NULL AND e.error != '')".into());
        }
        if let Some((cond, arg)) = status_where(&f.status) {
            wheres.push(cond.into());
            args.extend(arg.map(Value::from));
        }
        if !f.text.trim().is_empty() {
            wheres.push(
                "(e.model LIKE ? OR COALESCE(u.label, '') LIKE ? OR COALESCE(e.error, '') LIKE ?)"
                    .into(),
            );
            let needle = format!("%{}%", f.text.trim());
            args.extend(std::iter::repeat_n(Value::from(needle), 3));
        }
        let where_sql = wheres.join(" AND ");
        let conn = self.store.conn();
        let total: i64 = conn.query_row(
            &format!("SELECT COUNT(*) FROM usage_events e LEFT JOIN users u ON u.id = e.user_id WHERE {where_sql}"),
            params_from_iter(args.iter()),
            |r| r.get(0),
        )?;
        let mut list_args = args.clone();
        list_args.push(limit.into());
        list_args.push(f.offset.max(0).into());
        let mut stmt = conn.prepare(&format!(
            "SELECT e.id, e.ts, e.user_id, COALESCE(u.label, '(deleted)'), e.key_id, e.account_id, e.model,
                    e.input_tokens, e.output_tokens, e.cache_read_tokens, e.cache_creation_tokens,
                    e.latency_ms, e.status, COALESCE(e.error, '')
             FROM usage_events e LEFT JOIN users u ON u.id = e.user_id
             WHERE {where_sql} ORDER BY {} LIMIT ? OFFSET ?",
            sort_clause(&f.sort)
        ))?;
        let rows = stmt.query_map(params_from_iter(list_args.iter()), |r| {
            Ok(EventRow {
                id: r.get(0)?,
                ts: r.get(1)?,
                user_id: r.get(2)?,
                user_label: r.get(3)?,
                key_id: r.get(4)?,
                account_id: r.get(5)?,
                model: r.get(6)?,
                input_tokens: r.get(7)?,
                output_tokens: r.get(8)?,
                cache_read_tokens: r.get(9)?,
                cache_creation_tokens: r.get(10)?,
                latency_ms: r.get(11)?,
                status: r.get(12)?,
                error: r.get(13)?,
            })
        })?;
        Ok((rows.collect::<rusqlite::Result<_>>()?, total))
    }
}

fn run(store: Store, rx: Receiver<Msg>) {
    for msg in rx {
        match msg {
            Msg::Event(e) => {
                if let Err(err) = write(&store, &e) {
                    tracing::error!("stats: write failed: {err:#}");
                }
            }
            Msg::Flush(done) => {
                let _ = done.send(());
            }
        }
    }
}

fn write(store: &Store, e: &Event) -> Result<()> {
    store.conn().execute(
        "INSERT INTO usage_events(ts, user_id, key_id, account_id, model,
            input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
            latency_ms, status, error)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            e.ts,
            e.user_id,
            e.key_id,
            (e.account_id != 0).then_some(e.account_id),
            e.model,
            e.input_tokens,
            e.output_tokens,
            e.cache_read_tokens,
            e.cache_creation_tokens,
            e.latency_ms,
            e.status,
            (!e.error.is_empty()).then_some(&e.error),
        ],
    )?;
    Ok(())
}

fn since(days: i64) -> i64 {
    crate::now_unix() - days * 86_400
}

/// `ok` (2xx/3xx), `error` (>=400), `2xx`…`5xx` buckets, or a literal code.
fn status_where(s: &str) -> Option<(&'static str, Option<i64>)> {
    match s.trim().to_ascii_lowercase().as_str() {
        "" | "any" => None,
        "ok" => Some(("(e.status >= 200 AND e.status < 400)", None)),
        "error" => Some(("e.status >= 400", None)),
        "2xx" => Some(("(e.status >= 200 AND e.status < 300)", None)),
        "3xx" => Some(("(e.status >= 300 AND e.status < 400)", None)),
        "4xx" => Some(("(e.status >= 400 AND e.status < 500)", None)),
        "5xx" => Some(("e.status >= 500", None)),
        other => other
            .parse::<i64>()
            .ok()
            .map(|code| ("e.status = ?", Some(code))),
    }
}

fn sort_clause(s: &str) -> &'static str {
    match s {
        "time_asc" => "e.id ASC",
        "tokens_desc" => "(e.input_tokens + e.output_tokens) DESC, e.id DESC",
        "tokens_asc" => "(e.input_tokens + e.output_tokens) ASC, e.id ASC",
        "latency_desc" => "e.latency_ms DESC, e.id DESC",
        "latency_asc" => "e.latency_ms ASC, e.id ASC",
        _ => "e.id DESC",
    }
}

#[derive(Debug, Clone, Default)]
pub struct SearchFilter {
    /// Free text matched against model, user label and error.
    pub text: String,
    pub user_id: i64,
    pub account_id: i64,
    /// Substring of the model id.
    pub model: String,
    pub status: String,
    pub since: Option<i64>,
    pub until: Option<i64>,
    pub min_tokens: i64,
    pub only_error: bool,
    /// time_desc (default) | time_asc | tokens_desc | tokens_asc | latency_desc | latency_asc
    pub sort: String,
    /// Page size; defaults to 50, capped at 500.
    pub limit: i64,
    pub offset: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct EventRow {
    pub id: i64,
    pub ts: i64,
    pub user_id: i64,
    pub user_label: String,
    pub key_id: i64,
    pub account_id: Option<i64>,
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_creation_tokens: i64,
    pub latency_ms: i64,
    pub status: i64,
    pub error: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DailySummary {
    pub date: String,
    pub requests: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct UserSummary {
    pub user_id: i64,
    pub label: String,
    pub requests: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelSummary {
    pub model: String,
    pub requests: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct AccountSummary {
    pub account_id: i64,
    pub label: String,
    pub requests: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::Keys;

    fn seeded() -> (Recorder, Store, i64, i64) {
        let store = Store::open_in_memory().unwrap();
        let keys = Keys::new(store.clone());
        let alice = keys.create_user("alice").unwrap();
        let bob = keys.create_user("bob").unwrap();
        let rec = Recorder::new(store.clone());
        let now = crate::now_unix();
        let ev = |ago: i64, user, model: &str, i, o, lat, status, err: &str| Event {
            ts: now - ago,
            user_id: user,
            key_id: 1,
            model: model.into(),
            input_tokens: i,
            output_tokens: o,
            latency_ms: lat,
            status,
            error: err.into(),
            ..Default::default()
        };
        for e in [
            ev(600, alice, "claude-opus-4-7", 100, 200, 1500, 200, ""),
            ev(540, alice, "claude-haiku-4-5", 10, 20, 200, 200, ""),
            ev(
                480,
                bob,
                "claude-opus-4-7",
                50,
                75,
                800,
                429,
                "rate_limit_error: Error (req_id=req_x)",
            ),
            ev(420, bob, "claude-sonnet-4-6", 5000, 8000, 30000, 200, ""),
            ev(
                360,
                alice,
                "claude-opus-4-7",
                0,
                0,
                50,
                401,
                "authentication_error",
            ),
        ] {
            rec.record(e);
        }
        rec.flush();
        (rec, store, alice, bob)
    }

    fn count(rec: &Recorder, f: SearchFilter) -> i64 {
        rec.search(&f).unwrap().1
    }

    #[test]
    fn search_filters() {
        let (rec, _, alice, _) = seeded();
        assert_eq!(count(&rec, SearchFilter::default()), 5);
        assert_eq!(
            count(
                &rec,
                SearchFilter {
                    user_id: alice,
                    ..Default::default()
                }
            ),
            3
        );
        assert_eq!(
            count(
                &rec,
                SearchFilter {
                    model: "opus".into(),
                    ..Default::default()
                }
            ),
            3
        );
        assert_eq!(
            count(
                &rec,
                SearchFilter {
                    status: "error".into(),
                    ..Default::default()
                }
            ),
            2
        );
        assert_eq!(
            count(
                &rec,
                SearchFilter {
                    status: "429".into(),
                    ..Default::default()
                }
            ),
            1
        );
        assert_eq!(
            count(
                &rec,
                SearchFilter {
                    status: "ok".into(),
                    ..Default::default()
                }
            ),
            3
        );
        assert_eq!(
            count(
                &rec,
                SearchFilter {
                    only_error: true,
                    ..Default::default()
                }
            ),
            2
        );
        assert_eq!(
            count(
                &rec,
                SearchFilter {
                    min_tokens: 1000,
                    ..Default::default()
                }
            ),
            1
        );
        assert_eq!(
            count(
                &rec,
                SearchFilter {
                    text: "bob".into(),
                    ..Default::default()
                }
            ),
            2
        );
        assert_eq!(
            count(
                &rec,
                SearchFilter {
                    text: "req_x".into(),
                    ..Default::default()
                }
            ),
            1
        );
    }

    #[test]
    fn search_sorting_and_pagination() {
        let (rec, _, _, _) = seeded();
        let (rows, _) = rec
            .search(&SearchFilter {
                sort: "tokens_desc".into(),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(rows[0].model, "claude-sonnet-4-6");
        let (rows, _) = rec
            .search(&SearchFilter {
                sort: "latency_asc".into(),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(rows[0].latency_ms, 50);
        let (page, total) = rec
            .search(&SearchFilter {
                limit: 2,
                offset: 4,
                ..Default::default()
            })
            .unwrap();
        assert_eq!((page.len(), total), (1, 5));
    }

    #[test]
    fn deleted_users_still_listed_and_rollups() {
        let (rec, store, _, bob) = seeded();
        store
            .conn()
            .execute("DELETE FROM users WHERE id = ?1", [bob])
            .unwrap();
        let (rows, total) = rec.search(&SearchFilter::default()).unwrap();
        assert_eq!(total, 5);
        assert!(rows.iter().any(|r| r.user_label == "(deleted)"));
        let models = rec.by_model(1).unwrap();
        assert_eq!(
            models
                .iter()
                .find(|m| m.model == "claude-opus-4-7")
                .unwrap()
                .requests,
            3
        );
        assert_eq!(
            rec.by_day(1)
                .unwrap()
                .iter()
                .map(|d| d.requests)
                .sum::<i64>(),
            5
        );
        assert_eq!(rec.by_user(1).unwrap().len(), 1);
    }
}
