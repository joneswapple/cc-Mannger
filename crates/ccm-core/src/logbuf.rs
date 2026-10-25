//! The most recent log lines, kept in memory for the admin log viewer.
//! Install [`layer()`] in the host's tracing subscriber; nothing here ever
//! sees a secret that isn't already kept out of the logs.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::{Mutex, OnceLock};

use serde::Serialize;
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

const CAPACITY: usize = 2000;

#[derive(Debug, Clone, Serialize)]
pub struct LogLine {
    pub seq: u64,
    pub ts_ms: i64,
    pub level: String,
    pub target: String,
    pub message: String,
}

#[derive(Default)]
struct Inner {
    lines: VecDeque<LogLine>,
    next: u64,
}

pub struct LogBuffer {
    inner: Mutex<Inner>,
}

/// The process-wide buffer.
pub fn buffer() -> &'static LogBuffer {
    static B: OnceLock<LogBuffer> = OnceLock::new();
    B.get_or_init(|| LogBuffer {
        inner: Mutex::new(Inner::default()),
    })
}

/// A tracing layer that copies every event into [`buffer()`].
pub fn layer() -> BufferLayer {
    BufferLayer
}

fn rank(level: &str) -> u8 {
    match level {
        "ERROR" => 4,
        "WARN" => 3,
        "INFO" => 2,
        "DEBUG" => 1,
        _ => 0,
    }
}

impl LogBuffer {
    pub fn push(&self, level: &Level, target: &str, message: String) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.next += 1;
        let line = LogLine {
            seq: inner.next,
            ts_ms: crate::now_millis(),
            level: level.to_string(),
            target: target.to_string(),
            message,
        };
        if inner.lines.len() >= CAPACITY {
            inner.lines.pop_front();
        }
        inner.lines.push_back(line);
    }

    /// Lines after `after` at or above `min_level` (`ERROR` … `TRACE`),
    /// oldest first, at most `limit` (the newest ones).
    pub fn since(&self, after: u64, min_level: &str, limit: usize) -> Vec<LogLine> {
        let min = rank(&min_level.to_ascii_uppercase());
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let mut out: Vec<LogLine> = inner
            .lines
            .iter()
            .rev()
            .filter(|l| l.seq > after && rank(&l.level) >= min)
            .take(limit.clamp(1, CAPACITY))
            .cloned()
            .collect();
        out.reverse();
        out
    }

    pub fn clear(&self) {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .lines
            .clear();
    }
}

pub struct BufferLayer;

#[derive(Default)]
struct Message {
    text: String,
    fields: String,
}

impl Visit for Message {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.text, "{value:?}");
        } else {
            let _ = write!(self.fields, " {}={value:?}", field.name());
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.text.push_str(value);
        } else {
            let _ = write!(self.fields, " {}={value}", field.name());
        }
    }
}

impl<S: Subscriber> Layer<S> for BufferLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        let mut m = Message::default();
        event.record(&mut m);
        buffer().push(
            meta.level(),
            meta.target(),
            format!("{}{}", m.text, m.fields),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::layer::SubscriberExt;

    #[test]
    fn captures_events_with_fields_and_filters_by_level() {
        let sub = tracing_subscriber::registry().with(layer());
        let before = buffer()
            .since(0, "TRACE", CAPACITY)
            .last()
            .map_or(0, |l| l.seq);
        tracing::subscriber::with_default(sub, || {
            tracing::info!(account = 3, "picked");
            tracing::warn!("cooling down");
        });
        let lines = buffer().since(before, "TRACE", 100);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].message, "picked account=3");
        assert_eq!(buffer().since(before, "warn", 100).len(), 1);
    }
}
