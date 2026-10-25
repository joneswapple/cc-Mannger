//! Extracting token usage and Anthropic error envelopes from upstream
//! responses while the bytes pass through untouched.

use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use futures_util::Stream;
use serde::Deserialize;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Usage {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_creation_input_tokens: i64,
    pub cache_read_input_tokens: i64,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Captured {
    pub usage: Usage,
    pub error_type: String,
    pub error_message: String,
    pub request_id: String,
    /// The first bytes of the body, when a body limit was set (monitor).
    pub body: Vec<u8>,
    /// When the first body chunk arrived (streams only).
    pub first_chunk: Option<std::time::Instant>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ErrorBody {
    #[serde(rename = "type")]
    kind: String,
    message: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Envelope {
    #[serde(rename = "type")]
    kind: String,
    usage: Usage,
    error: ErrorBody,
    request_id: String,
    message: Option<MessageStart>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct MessageStart {
    usage: Usage,
}

impl Captured {
    /// Parses a complete non-streaming response body.
    pub fn from_json(body: &[u8]) -> Self {
        let env: Envelope = serde_json::from_slice(body).unwrap_or_default();
        let mut c = Captured {
            usage: env.usage,
            request_id: env.request_id,
            ..Default::default()
        };
        if env.kind == "error" || !env.error.kind.is_empty() {
            c.error_type = env.error.kind;
            c.error_message = env.error.message;
        }
        c
    }

    /// Short summary for logs and `usage_events.error`; empty when the
    /// response was a success.
    pub fn error_summary(&self, status: u16) -> String {
        if status < 400 && self.error_type.is_empty() {
            return String::new();
        }
        let kind = non_empty(&self.error_type, "upstream_error");
        let reason = reqwest::StatusCode::from_u16(status)
            .ok()
            .and_then(|s| s.canonical_reason())
            .unwrap_or("error");
        let mut s = format!("{kind}: {}", non_empty(&self.error_message, reason));
        if !self.request_id.is_empty() {
            s.push_str(&format!(" (req_id={})", self.request_id));
        }
        s
    }
}

fn non_empty<'a>(v: &'a str, fallback: &'a str) -> &'a str {
    if v.trim().is_empty() {
        fallback
    } else {
        v
    }
}

/// Incremental SSE parser: feed raw chunks, it tracks `event:` / `data:`
/// lines across chunk boundaries.
#[derive(Default)]
pub struct SseCapture {
    pending: Vec<u8>,
    event: String,
    pub captured: Captured,
}

impl SseCapture {
    pub fn feed(&mut self, chunk: &[u8]) {
        self.pending.extend_from_slice(chunk);
        while let Some(pos) = self.pending.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=pos).collect();
            self.line(&line);
        }
    }

    /// Processes a trailing line with no final newline.
    pub fn finish(&mut self) {
        if !self.pending.is_empty() {
            let line = std::mem::take(&mut self.pending);
            self.line(&line);
        }
    }

    fn line(&mut self, raw: &[u8]) {
        let line = String::from_utf8_lossy(raw);
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            self.event.clear();
            return;
        }
        if let Some(ev) = line.strip_prefix("event:") {
            self.event = ev.trim().to_string();
            return;
        }
        let Some(data) = line.strip_prefix("data:") else {
            return;
        };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            return;
        }
        let Ok(env) = serde_json::from_str::<Envelope>(data) else {
            return;
        };
        let u = &mut self.captured.usage;
        match self.event.as_str() {
            "message_start" => {
                let m = env.message.unwrap_or_default().usage;
                set_if_positive(&mut u.input_tokens, m.input_tokens);
                set_if_positive(
                    &mut u.cache_creation_input_tokens,
                    m.cache_creation_input_tokens,
                );
                set_if_positive(&mut u.cache_read_input_tokens, m.cache_read_input_tokens);
            }
            "message_delta" => {
                set_if_positive(&mut u.output_tokens, env.usage.output_tokens);
                set_if_positive(&mut u.input_tokens, env.usage.input_tokens);
            }
            "error" => {
                if !env.error.kind.is_empty() {
                    self.captured.error_type = env.error.kind;
                    self.captured.error_message = env.error.message;
                }
                if !env.request_id.is_empty() {
                    self.captured.request_id = env.request_id;
                }
            }
            _ => {}
        }
    }
}

fn set_if_positive(dst: &mut i64, v: i64) {
    if v > 0 {
        *dst = v;
    }
}

type ByteStream = Pin<Box<dyn Stream<Item = reqwest::Result<Bytes>> + Send>>;

/// Wraps an upstream SSE byte stream: forwards every chunk as-is and calls
/// `on_done` exactly once with what was captured — when the stream ends,
/// errors, or the client disconnects (drop).
pub struct TeeStream {
    inner: ByteStream,
    sse: SseCapture,
    on_done: Option<Box<dyn FnOnce(Captured) + Send>>,
    /// Bytes of the body to keep in `Captured::body`.
    keep: usize,
}

impl TeeStream {
    pub fn new(inner: ByteStream, on_done: impl FnOnce(Captured) + Send + 'static) -> Self {
        Self {
            inner,
            sse: SseCapture::default(),
            on_done: Some(Box::new(on_done)),
            keep: 0,
        }
    }

    /// Also keeps up to `limit` bytes of the body for the caller.
    pub fn keep_body(mut self, limit: usize) -> Self {
        self.keep = limit;
        self
    }

    fn done(&mut self) {
        if let Some(f) = self.on_done.take() {
            self.sse.finish();
            f(std::mem::take(&mut self.sse.captured));
        }
    }
}

impl Stream for TeeStream {
    type Item = reqwest::Result<Bytes>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.inner.as_mut().poll_next(cx) {
            Poll::Ready(Some(Ok(chunk))) => {
                self.sse.feed(&chunk);
                let c = &mut self.sse.captured;
                c.first_chunk.get_or_insert_with(std::time::Instant::now);
                let room = self.keep.saturating_sub(self.sse.captured.body.len());
                if room > 0 {
                    let n = room.min(chunk.len());
                    self.sse.captured.body.extend_from_slice(&chunk[..n]);
                }
                Poll::Ready(Some(Ok(chunk)))
            }
            Poll::Ready(Some(Err(e))) => {
                tracing::warn!("proxy: stream read: {e}");
                self.done();
                Poll::Ready(Some(Err(e)))
            }
            Poll::Ready(None) => {
                self.done();
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for TeeStream {
    fn drop(&mut self) {
        self.done();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_usage_across_chunk_boundaries() {
        let stream = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"m1\",\"usage\":{\"input_tokens\":10,\"cache_read_input_tokens\":2}}}\n\n",
            "event: message_delta\r\n",
            "data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":5}}\r\n\r\n",
        );
        let mut c = SseCapture::default();
        for chunk in stream.as_bytes().chunks(7) {
            c.feed(chunk);
        }
        c.finish();
        assert_eq!(
            c.captured.usage,
            Usage {
                input_tokens: 10,
                output_tokens: 5,
                cache_read_input_tokens: 2,
                cache_creation_input_tokens: 0
            }
        );
        assert_eq!(c.captured.error_summary(200), "");
    }

    #[test]
    fn sse_error_event() {
        let mut c = SseCapture::default();
        c.feed(b"event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"},\"request_id\":\"req_1\"}");
        c.finish();
        assert_eq!(
            c.captured.error_summary(200),
            "overloaded_error: Overloaded (req_id=req_1)"
        );
    }

    #[test]
    fn json_body() {
        let c = Captured::from_json(br#"{"type":"error","error":{"type":"rate_limit_error","message":"Error"},"request_id":"req_x"}"#);
        assert_eq!(
            c.error_summary(429),
            "rate_limit_error: Error (req_id=req_x)"
        );
        let c = Captured::from_json(br#"{"id":"m","usage":{"input_tokens":7,"output_tokens":3}}"#);
        assert_eq!((c.usage.input_tokens, c.usage.output_tokens), (7, 3));
        assert_eq!(
            Captured::from_json(b"not json").error_summary(502),
            "upstream_error: Bad Gateway"
        );
    }
}
