//! The request monitor: the most recent proxied requests with their bodies,
//! for debugging what a terminal sent, what went upstream and what came
//! back. In memory only and off by default (bodies hold prompts); usage
//! numbers are recorded permanently by `stats.rs` either way.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, RwLock};

use serde::Serialize;

use crate::config::MonitorConfig;
use crate::upstream::Attempt;

/// Everything about one request except the bodies.
#[derive(Debug, Clone, Default, Serialize)]
pub struct RequestMeta {
    pub id: u64,
    /// Unix milliseconds when the request arrived.
    pub ts_ms: i64,
    /// `/v1/messages` or `/v1/chat/completions`.
    pub endpoint: String,
    pub client_ip: String,
    pub user_id: i64,
    pub user_label: String,
    pub key_prefix: String,
    /// The model the client asked for, and the one sent upstream after
    /// routing.
    pub model: String,
    pub mapped_model: String,
    pub stream: bool,
    pub account_id: Option<i64>,
    pub status: u16,
    pub latency_ms: i64,
    /// Time to the first response byte (first SSE chunk when streaming).
    pub ttft_ms: Option<i64>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_creation_tokens: i64,
    /// Attempts that failed over before the final answer.
    pub failover: Vec<Attempt>,
    pub error: String,
}

/// Bodies as text, each cut to `max_body_bytes`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Bodies {
    pub request: String,
    pub upstream_request: String,
    pub response: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct RequestLog {
    #[serde(flatten)]
    pub meta: RequestMeta,
    #[serde(flatten)]
    pub bodies: Bodies,
}

/// List filter; empty fields match everything.
#[derive(Debug, Default, Clone)]
pub struct Filter {
    /// Only entries with an id above this (for polling).
    pub after_id: u64,
    /// Substring of model, mapped model, terminal label, error or IP.
    pub q: String,
    /// `ok`, `error`, or an exact status code.
    pub status: String,
    pub account_id: Option<i64>,
    pub limit: usize,
}

#[derive(Default)]
struct Inner {
    buf: VecDeque<RequestLog>,
    next_id: u64,
}

#[derive(Clone)]
pub struct Monitor {
    inner: Arc<Mutex<Inner>>,
    cfg: Arc<RwLock<MonitorConfig>>,
}

impl Monitor {
    pub fn new(cfg: MonitorConfig) -> Self {
        Self {
            inner: Default::default(),
            cfg: Arc::new(RwLock::new(cfg)),
        }
    }

    pub fn config(&self) -> MonitorConfig {
        self.cfg.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Applies new settings; a smaller capacity drops the oldest entries.
    pub fn set_config(&self, cfg: MonitorConfig) {
        let cap = cfg.capacity;
        *self.cfg.write().unwrap_or_else(|e| e.into_inner()) = cfg;
        let mut inner = self.lock();
        while inner.buf.len() > cap {
            inner.buf.pop_front();
        }
    }

    /// Bytes of each body to keep, or `None` when recording is off.
    pub fn body_limit(&self) -> Option<usize> {
        let c = self.config();
        c.enabled.then_some(c.max_body_bytes)
    }

    /// Stores a finished request, assigning its id.
    pub fn record(&self, mut meta: RequestMeta, bodies: Bodies) -> u64 {
        let cap = self.config().capacity.max(1);
        let mut inner = self.lock();
        inner.next_id += 1;
        meta.id = inner.next_id;
        while inner.buf.len() >= cap {
            inner.buf.pop_front();
        }
        inner.buf.push_back(RequestLog { meta, bodies });
        inner.next_id
    }

    /// Matching entries, newest first, plus the number of entries held.
    pub fn list(&self, f: &Filter) -> (Vec<RequestMeta>, usize) {
        let inner = self.lock();
        let q = f.q.trim().to_lowercase();
        let limit = if f.limit == 0 { 200 } else { f.limit.min(1000) };
        let items = inner
            .buf
            .iter()
            .rev()
            .map(|l| &l.meta)
            .filter(|m| m.id > f.after_id)
            .filter(|m| f.account_id.is_none_or(|a| m.account_id == Some(a)))
            .filter(|m| match f.status.as_str() {
                "" => true,
                "ok" => m.status < 400,
                "error" => m.status >= 400,
                code => code.parse::<u16>().is_ok_and(|c| c == m.status),
            })
            .filter(|m| {
                q.is_empty()
                    || [
                        &m.model,
                        &m.mapped_model,
                        &m.user_label,
                        &m.error,
                        &m.client_ip,
                    ]
                    .iter()
                    .any(|s| s.to_lowercase().contains(&q))
            })
            .take(limit)
            .cloned()
            .collect();
        (items, inner.buf.len())
    }

    pub fn get(&self, id: u64) -> Option<RequestLog> {
        self.lock().buf.iter().find(|l| l.meta.id == id).cloned()
    }

    pub fn clear(&self) {
        self.lock().buf.clear();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// `bytes` as text, cut to `limit` bytes on a character boundary. Returns
/// whether anything was cut.
pub fn body_text(bytes: &[u8], limit: usize) -> (String, bool) {
    let text = String::from_utf8_lossy(bytes);
    if text.len() <= limit {
        return (text.into_owned(), false);
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_string(), true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(model: &str, status: u16) -> RequestMeta {
        RequestMeta {
            model: model.into(),
            status,
            ..Default::default()
        }
    }

    #[test]
    fn ring_buffer_filters_and_shrinks() {
        let m = Monitor::new(MonitorConfig {
            enabled: true,
            capacity: 3,
            max_body_bytes: 10,
        });
        for (i, s) in [200, 429, 200, 500].into_iter().enumerate() {
            m.record(meta(&format!("m{i}"), s), Bodies::default());
        }
        let (all, held) = m.list(&Filter::default());
        assert_eq!(held, 3);
        assert_eq!(
            all.iter().map(|x| x.id).collect::<Vec<_>>(),
            [4, 3, 2],
            "newest first, oldest dropped"
        );
        let errs = m.list(&Filter {
            status: "error".into(),
            ..Default::default()
        });
        assert_eq!(errs.0.len(), 2);
        let newer = m.list(&Filter {
            after_id: 3,
            ..Default::default()
        });
        assert_eq!(newer.0.len(), 1);
        assert!(m.get(2).is_some() && m.get(1).is_none());
        m.set_config(MonitorConfig {
            enabled: true,
            capacity: 1,
            max_body_bytes: 10,
        });
        assert_eq!(m.list(&Filter::default()).1, 1);
    }

    #[test]
    fn body_text_cuts_on_char_boundary() {
        assert_eq!(body_text("héllo".as_bytes(), 2), ("h".into(), true));
        assert_eq!(body_text(b"abc", 10), ("abc".into(), false));
    }
}
