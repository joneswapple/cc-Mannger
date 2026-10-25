//! `POST /v1/messages`, `POST /v1/chat/completions` (OpenAI-compatible),
//! `POST /v1/messages/count_tokens`, `GET /v1/models`.

use std::time::Instant;

use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, HeaderName, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use reqwest::Method;
use serde::Deserialize;
use serde_json::json;

use super::{anthropic_error, header, ApiIdentity, ClientIp};
use crate::auth::Identity;
use crate::capture::{Captured, TeeStream};
use crate::monitor::{self, Bodies, Monitor, RequestMeta};
use crate::policy::Denied;
use crate::pool::{AllCooling, NoAccount, PickRequest};
use crate::stats::Event;
use crate::upstream::{passthrough_user_agent, Attempt, ForwardError, ForwardExtra, Forwarded};
use crate::Core;
use crate::{openai, rewrite};
use futures_util::StreamExt;

#[derive(Deserialize)]
struct MessageRequest {
    #[serde(default)]
    model: String,
    #[serde(default)]
    stream: Option<bool>,
    #[serde(default)]
    metadata: Option<Metadata>,
}

#[derive(Deserialize)]
struct Metadata {
    #[serde(default)]
    user_id: Option<String>,
}

/// Session identity for sticky routing: Claude Code puts a per-session id
/// in `metadata.user_id`; other clients fall back to their terminal key.
fn sticky_key(req: &MessageRequest, id: &Identity) -> String {
    match req.metadata.as_ref().and_then(|m| m.user_id.as_deref()) {
        Some(u) if !u.is_empty() => format!("session:{u}"),
        _ => format!("key:{}", id.key_id),
    }
}

/// A request turned away before (or instead of) forwarding an upstream
/// answer. Becomes an Anthropic-shaped error response.
struct Reject {
    status: StatusCode,
    kind: &'static str,
    message: String,
    retry_after: Option<i64>,
}

impl Reject {
    fn new(status: StatusCode, kind: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            kind,
            message: message.into(),
            retry_after: None,
        }
    }

    fn internal(e: impl std::fmt::Display) -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "api_error",
            e.to_string(),
        )
    }

    fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_request_error", message)
    }

    fn into_response(self) -> Response {
        let mut resp = anthropic_error(self.status, self.kind, self.message);
        if let Some(v) = self.retry_after.and_then(|r| r.to_string().parse().ok()) {
            resp.headers_mut().insert("retry-after", v);
        }
        resp
    }
}

/// Client-facing error when no upstream answer can be forwarded.
fn forward_error(e: &ForwardError) -> Reject {
    match e {
        ForwardError::Pick(err) => {
            if let Some(c) = err.downcast_ref::<AllCooling>() {
                Reject {
                    retry_after: Some(c.retry_after),
                    ..Reject::new(
                        StatusCode::TOO_MANY_REQUESTS,
                        "rate_limit_error",
                        c.to_string(),
                    )
                }
            } else if err.is::<NoAccount>() {
                Reject::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "api_error",
                    format!("upstream: {err}"),
                )
            } else {
                Reject::internal(format!("{err:#}"))
            }
        }
        ForwardError::Exhausted { last, .. } => Reject::new(
            StatusCode::BAD_GATEWAY,
            "api_error",
            format!("upstream: {}", last.error),
        ),
        ForwardError::Response(_) => Reject::internal("unforwarded upstream response"),
    }
}

/// The failed attempts behind a forward outcome, for stats.
fn failed_attempts(r: &Result<Forwarded, ForwardError>) -> Vec<Attempt> {
    match r {
        Ok(f) => f.failed.clone(),
        Err(ForwardError::Exhausted { last, failed }) => failed
            .iter()
            .chain(std::iter::once(last))
            .cloned()
            .collect(),
        Err(_) => vec![],
    }
}

fn forward_extra(headers: &HeaderMap) -> ForwardExtra {
    ForwardExtra {
        extra_beta: header(headers, "anthropic-beta").map(String::from),
        user_agent: header(headers, "user-agent").and_then(passthrough_user_agent),
        content_type: header(headers, "content-type").map(String::from),
    }
}

/// Prepends the Claude Code system block when enabled; a body that can't be
/// rewritten is forwarded as is.
pub(super) fn maybe_inject(core: &Core, body: Bytes) -> Bytes {
    if !core.config.upstream.inject_claude_code_system {
        return body;
    }
    match rewrite::inject_claude_code_system(&body) {
        Ok(Some(b)) => b.into(),
        Ok(None) => body,
        Err(e) => {
            tracing::warn!("proxy: system-prompt injection skipped: {e:#}");
            body
        }
    }
}

/// Copies upstream status and headers, minus hop-by-hop ones and the
/// length (bodies may be re-chunked or decompressed).
fn response_head(resp: &reqwest::Response) -> axum::http::response::Builder {
    let mut b = Response::builder().status(resp.status().as_u16());
    for (k, v) in resp.headers() {
        if matches!(
            k.as_str(),
            "connection" | "keep-alive" | "transfer-encoding" | "upgrade" | "content-length"
        ) {
            continue;
        }
        if let Ok(name) = HeaderName::from_bytes(k.as_str().as_bytes()) {
            b = b.header(name, v.as_bytes());
        }
    }
    b
}

/// Where a request came from, for the monitor.
struct Origin {
    endpoint: &'static str,
    ip: String,
    /// The body as the client sent it (before any translation).
    body: Bytes,
}

/// A monitor entry being filled in while a request is handled.
struct Draft {
    meta: RequestMeta,
    bodies: Bodies,
    limit: usize,
    arrived: Instant,
}

impl Draft {
    fn new(core: &Core, id: &Identity, origin: &Origin, model: &str, stream: bool) -> Option<Self> {
        let limit = core.monitor.body_limit()?;
        let (request, truncated) = monitor::body_text(&origin.body, limit);
        Some(Draft {
            meta: RequestMeta {
                ts_ms: crate::now_millis(),
                endpoint: origin.endpoint.into(),
                client_ip: origin.ip.clone(),
                user_id: id.user_id,
                user_label: id.user_label.clone(),
                key_prefix: id.key_prefix.clone(),
                model: model.into(),
                mapped_model: model.into(),
                stream,
                ..Default::default()
            },
            bodies: Bodies {
                request,
                truncated,
                ..Default::default()
            },
            limit,
            arrived: Instant::now(),
        })
    }

    fn set_upstream_body(&mut self, body: &[u8]) {
        let (text, cut) = monitor::body_text(body, self.limit);
        self.bodies.upstream_request = text;
        self.bodies.truncated |= cut;
    }

    fn reject(mut self, core: &Core, r: &Reject) {
        self.meta.status = r.status.as_u16();
        self.meta.error = r.message.clone();
        self.meta.latency_ms = self.arrived.elapsed().as_millis() as i64;
        core.monitor.record(self.meta, self.bodies);
    }

    fn finish(
        mut self,
        monitor: &Monitor,
        status: u16,
        c: &Captured,
        error: String,
        headers_at: Instant,
    ) {
        let m = &mut self.meta;
        m.status = status;
        m.error = error;
        m.latency_ms = self.arrived.elapsed().as_millis() as i64;
        m.ttft_ms = Some(
            c.first_chunk
                .unwrap_or(headers_at)
                .saturating_duration_since(self.arrived)
                .as_millis() as i64,
        );
        m.input_tokens = c.usage.input_tokens;
        m.output_tokens = c.usage.output_tokens;
        m.cache_read_tokens = c.usage.cache_read_input_tokens;
        m.cache_creation_tokens = c.usage.cache_creation_input_tokens;
        let (text, cut) = monitor::body_text(&c.body, self.limit);
        self.bodies.response = text;
        self.bodies.truncated |= cut || c.body.len() >= self.limit;
        monitor.record(self.meta, self.bodies);
    }
}

/// An upstream answer to a messages call, plus the hook that records it
/// once the body has been consumed.
struct Dispatched {
    resp: reqwest::Response,
    status: u16,
    finish: Box<dyn FnOnce(Captured) + Send>,
    /// Bytes of the response body to capture for the monitor (0 = none).
    keep: usize,
}

/// The shared `/v1/messages` pipeline: model routing, effort tiers, policy,
/// system injection, failover forwarding, and stats for failed attempts.
/// Errors are client-ready Anthropic-shaped responses; every outcome is
/// also offered to the request monitor.
async fn dispatch(
    core: &Core,
    id: Identity,
    origin: Origin,
    headers: &HeaderMap,
    body: Bytes,
) -> Result<Dispatched, Response> {
    let mut draft = None;
    match dispatch_inner(core, id, &origin, headers, body, &mut draft).await {
        Ok(d) => Ok(d),
        Err(r) => {
            if let Some(d) = draft {
                d.reject(core, &r);
            }
            Err(r.into_response())
        }
    }
}

async fn dispatch_inner(
    core: &Core,
    id: Identity,
    origin: &Origin,
    headers: &HeaderMap,
    body: Bytes,
    draft: &mut Option<Draft>,
) -> Result<Dispatched, Reject> {
    let mut value: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|e| Reject::bad_request(format!("parse body: {e}")))?;
    let parsed: MessageRequest = serde_json::from_value(value.clone()).unwrap_or(MessageRequest {
        model: String::new(),
        stream: None,
        metadata: None,
    });
    *draft = Draft::new(
        core,
        &id,
        origin,
        &parsed.model,
        parsed.stream.unwrap_or(false),
    );
    if parsed.model.is_empty() {
        return Err(Reject::bad_request("model is required"));
    }
    let sticky = sticky_key(&parsed, &id);

    // Model routes first: policy and stats see the model actually used.
    let model = core
        .routing
        .resolve(&parsed.model, &value)
        .map_err(|e| Reject::internal(format!("routing: {e:#}")))?;
    if let Some(d) = draft.as_mut() {
        d.meta.mapped_model = model.clone();
    }
    let body = if model != parsed.model {
        value["model"] = serde_json::json!(model);
        Bytes::from(serde_json::to_vec(&value).map_err(Reject::internal)?)
    } else {
        body
    };

    // Effort tiers next, so later steps see a canonical body.
    let body = match rewrite::apply_effort(&body, &core.config.upstream.effort_levels) {
        Ok(Some(b)) => Bytes::from(b),
        Ok(None) => body,
        Err(e) => return Err(Reject::bad_request(e.to_string())),
    };

    let denied = |d: Denied| {
        let (status, kind) = match d {
            Denied::RateLimited | Denied::QuotaExceeded => {
                (StatusCode::TOO_MANY_REQUESTS, "rate_limit_error")
            }
            Denied::ModelDisabled | Denied::ModelNotAllowed | Denied::TooManyIps(_) => {
                (StatusCode::FORBIDDEN, "permission_error")
            }
        };
        Reject::new(status, kind, d.to_string())
    };
    let policy = match core.policy.check(id.user_id, &model) {
        Ok(Ok(p)) => p,
        Ok(Err(d)) => return Err(denied(d)),
        Err(e) => return Err(Reject::internal(format!("policy: {e:#}"))),
    };
    match core.policy.track_ip(id.user_id, &origin.ip, policy.max_ips) {
        Ok(Ok(())) => {}
        Ok(Err(d)) => return Err(denied(d)),
        Err(e) => return Err(Reject::internal(format!("policy: {e:#}"))),
    }

    let body = maybe_inject(core, body);
    if let Some(d) = draft.as_mut() {
        d.set_upstream_body(&body);
    }
    let start = Instant::now();
    let event = move |account_id: i64, status: u16, c: &Captured, error: String| Event {
        ts: crate::now_unix(),
        user_id: id.user_id,
        key_id: id.key_id,
        account_id,
        model: model.clone(),
        input_tokens: c.usage.input_tokens,
        output_tokens: c.usage.output_tokens,
        cache_read_tokens: c.usage.cache_read_input_tokens,
        cache_creation_tokens: c.usage.cache_creation_input_tokens,
        latency_ms: start.elapsed().as_millis() as i64,
        status,
        error,
    };

    let pick = PickRequest {
        sticky_key: Some(sticky),
        group: policy.account_group,
        exclude: vec![],
    };
    let result = core
        .upstream
        .forward(
            &pick,
            Method::POST,
            "/v1/messages",
            Some(body),
            &forward_extra(headers),
        )
        .await
        .or_else(ForwardError::into_forwarded);
    let failed = failed_attempts(&result);
    for a in &failed {
        core.recorder.record(Event {
            latency_ms: a.latency_ms,
            ..event(
                a.account_id,
                a.status,
                &Captured::default(),
                format!("failover: {}", a.error),
            )
        });
    }
    if let Some(d) = draft.as_mut() {
        d.meta.account_id = failed.last().map(|a| a.account_id);
        d.meta.failover = failed;
    }
    let Forwarded {
        resp, account_id, ..
    } = result.map_err(|e| forward_error(&e))?;
    let headers_at = Instant::now();
    let status = resp.status().as_u16();
    let header_str = |k: &str| {
        resp.headers()
            .get(k)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string()
    };
    let request_id_hdr = header_str("request-id");
    let retry_after = header_str("retry-after");

    let recorder = core.recorder.clone();
    let monitor = core.monitor.clone();
    let user = id_summary(&id);
    let mut draft = draft.take();
    if let Some(d) = draft.as_mut() {
        d.meta.account_id = Some(account_id);
    }
    let keep = draft.as_ref().map_or(0, |d| d.limit);
    let finish = move |mut c: Captured| {
        if c.request_id.is_empty() {
            c.request_id = request_id_hdr;
        }
        let summary = c.error_summary(status);
        if !summary.is_empty() {
            tracing::warn!(
                "upstream error: {user} account={account_id} status={status} type={} request_id={} retry_after={retry_after} message={:?}",
                c.error_type,
                c.request_id,
                c.error_message
            );
        }
        recorder.record(event(account_id, status, &c, summary.clone()));
        if let Some(d) = draft {
            d.finish(&monitor, status, &c, summary, headers_at);
        }
    };
    Ok(Dispatched {
        resp,
        status,
        finish: Box::new(finish),
        keep,
    })
}

fn is_sse(resp: &reqwest::Response) -> bool {
    resp.headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/event-stream"))
}

/// Reads a whole upstream body, recording it; read failures become 502s.
async fn buffered(d: Dispatched) -> Result<Bytes, Response> {
    match d.resp.bytes().await {
        Ok(buf) => {
            let mut c = Captured::from_json(&buf);
            c.body = buf[..buf.len().min(d.keep)].to_vec();
            (d.finish)(c);
            Ok(buf)
        }
        Err(e) => {
            (d.finish)(Captured {
                error_message: e.to_string(),
                ..Default::default()
            });
            Err(anthropic_error(
                StatusCode::BAD_GATEWAY,
                "api_error",
                format!("upstream: read body: {e}"),
            ))
        }
    }
}

pub(super) async fn messages(
    State(core): State<Core>,
    ApiIdentity(id): ApiIdentity,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let origin = Origin {
        endpoint: "/v1/messages",
        ip,
        body: body.clone(),
    };
    let d = match dispatch(&core, id, origin, &headers, body).await {
        Ok(d) => d,
        Err(resp) => return resp,
    };
    let head = response_head(&d.resp);
    if is_sse(&d.resp) {
        let stream = TeeStream::new(Box::pin(d.resp.bytes_stream()), d.finish).keep_body(d.keep);
        return head
            .body(Body::from_stream(stream))
            .unwrap_or_else(internal);
    }
    match buffered(d).await {
        Ok(buf) => head.body(Body::from(buf)).unwrap_or_else(internal),
        Err(resp) => resp,
    }
}

/// OpenAI-compatible chat completions over the same pipeline. Successful
/// responses are translated; errors keep Anthropic's
/// `{"error":{"type","message"}}` body, which OpenAI SDKs read as well.
pub(super) async fn chat_completions(
    State(core): State<Core>,
    ApiIdentity(id): ApiIdentity,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let origin = Origin {
        endpoint: "/v1/chat/completions",
        ip,
        body: body.clone(),
    };
    let req: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return anthropic_error(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                format!("parse body: {e}"),
            )
        }
    };
    let (anthropic, info) = match openai::to_anthropic(&req) {
        Ok(x) => x,
        Err(e) => {
            return anthropic_error(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                format!("{e:#}"),
            )
        }
    };
    let body = match serde_json::to_vec(&anthropic) {
        Ok(b) => Bytes::from(b),
        Err(e) => return internal(e),
    };
    // The client's headers describe an OpenAI request; only the UA carries over.
    let mut fwd_headers = HeaderMap::new();
    if let Some(ua) = headers.get("user-agent") {
        fwd_headers.insert("user-agent", ua.clone());
    }
    let d = match dispatch(&core, id, origin, &fwd_headers, body).await {
        Ok(d) => d,
        Err(resp) => return resp,
    };
    if d.status >= 400 {
        let mut head = Response::builder()
            .status(d.status)
            .header("content-type", "application/json");
        if let Some(v) = d.resp.headers().get("retry-after") {
            head = head.header("retry-after", v.clone());
        }
        return match buffered(d).await {
            Ok(buf) => head.body(Body::from(buf)).unwrap_or_else(internal),
            Err(resp) => resp,
        };
    }
    if is_sse(&d.resp) {
        let mut translator = openai::StreamTranslator::new(info);
        let stream = TeeStream::new(Box::pin(d.resp.bytes_stream()), d.finish)
            .keep_body(d.keep)
            .map(move |chunk| chunk.map(|b| Bytes::from(translator.feed(&b))));
        return Response::builder()
            .header("content-type", "text/event-stream")
            .header("cache-control", "no-cache")
            .body(Body::from_stream(stream))
            .unwrap_or_else(internal);
    }
    match buffered(d).await {
        Ok(buf) => match serde_json::from_slice::<serde_json::Value>(&buf) {
            Ok(v) => Json(openai::from_anthropic(&v, &info)).into_response(),
            Err(e) => internal(format!("decode upstream response: {e}")),
        },
        Err(resp) => resp,
    }
}

fn id_summary(id: &Identity) -> String {
    format!("user={} key={}", id.user_id, id.key_id)
}

fn internal(e: impl std::fmt::Display) -> Response {
    anthropic_error(
        StatusCode::INTERNAL_SERVER_ERROR,
        "api_error",
        e.to_string(),
    )
}

/// Same auth, headers and system injection as `/v1/messages`, but no policy
/// gate or stats: counting tokens isn't billable inference.
pub(super) async fn count_tokens(
    State(core): State<Core>,
    ApiIdentity(id): ApiIdentity,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // Count against the model the request would really be sent to.
    let body = match serde_json::from_slice::<serde_json::Value>(&body) {
        Ok(mut v) => match v["model"].as_str().map(String::from) {
            Some(m) => match core.routing.resolve(&m, &v) {
                Ok(target) if target != m => {
                    v["model"] = json!(target);
                    serde_json::to_vec(&v).map(Bytes::from).unwrap_or(body)
                }
                _ => body,
            },
            None => body,
        },
        Err(_) => body,
    };
    let body = maybe_inject(&core, body);
    let group = match core.policy.get_policy(id.user_id) {
        Ok(p) => p.account_group,
        Err(e) => return internal(format!("policy: {e:#}")),
    };
    let pick = PickRequest {
        sticky_key: Some(format!("key:{}", id.key_id)),
        group,
        exclude: vec![],
    };
    let resp = match core
        .upstream
        .forward(
            &pick,
            Method::POST,
            "/v1/messages/count_tokens",
            Some(body),
            &forward_extra(&headers),
        )
        .await
        .or_else(ForwardError::into_forwarded)
    {
        Ok(f) => f.resp,
        Err(e) => return forward_error(&e).into_response(),
    };
    let head = response_head(&resp);
    match resp.bytes().await {
        Ok(buf) => head.body(Body::from(buf)).unwrap_or_else(internal),
        Err(e) => anthropic_error(
            StatusCode::BAD_GATEWAY,
            "api_error",
            format!("upstream: read body: {e}"),
        ),
    }
}

/// Served from the cached catalog so clients can list models even when
/// they can't reach api.anthropic.com themselves.
pub(super) async fn models(State(core): State<Core>, ApiIdentity(_): ApiIdentity) -> Response {
    let data: Vec<_> = core
        .catalog
        .models()
        .into_iter()
        .map(|m| {
            // Anthropic fields plus OpenAI's, so both SDKs can list models.
            let created = m
                .created_at
                .as_deref()
                .and_then(|c| chrono::DateTime::parse_from_rfc3339(c).ok())
                .map_or(0, |t| t.timestamp());
            let mut row = json!({
                "type": "model", "id": m.id, "display_name": m.display_name,
                "object": "model", "created": created, "owned_by": "anthropic",
            });
            if let Some(c) = m.created_at {
                row["created_at"] = json!(c);
            }
            row
        })
        .collect();
    Json(json!({"object": "list", "data": data, "has_more": false})).into_response()
}

/// Admin connectivity test: one tiny request to `model` through the same
/// routing, system injection, pool and failover as terminal traffic. Not
/// recorded in usage stats or the monitor.
pub(super) async fn probe_model(core: &Core, model: &str) -> serde_json::Value {
    let start = Instant::now();
    let mut req = json!({
        "model": model,
        "max_tokens": 16,
        "messages": [{"role": "user", "content": "Reply with the single word: pong"}],
    });
    let mapped = core
        .routing
        .resolve(model, &req)
        .unwrap_or_else(|_| model.to_string());
    req["model"] = json!(mapped);
    let body = maybe_inject(core, Bytes::from(req.to_string()));
    let result = core
        .upstream
        .forward(
            &PickRequest::default(),
            Method::POST,
            "/v1/messages",
            Some(body),
            &ForwardExtra::default(),
        )
        .await
        .or_else(ForwardError::into_forwarded);
    let failover = failed_attempts(&result).len();
    let mut out = json!({"model": model, "mapped_model": mapped, "failover": failover});
    match result {
        Ok(f) => {
            let status = f.resp.status().as_u16();
            let buf = f.resp.bytes().await.unwrap_or_default();
            let v: serde_json::Value = serde_json::from_slice(&buf).unwrap_or_default();
            out["ok"] = json!(status < 400);
            out["status"] = json!(status);
            out["account_id"] = json!(f.account_id);
            out["text"] = v["content"][0]["text"].clone();
            out["error"] = json!(Captured::from_json(&buf).error_summary(status));
        }
        Err(e) => {
            let r = forward_error(&e);
            out["ok"] = json!(false);
            out["status"] = json!(r.status.as_u16());
            out["error"] = json!(r.message);
        }
    }
    out["latency_ms"] = json!(start.elapsed().as_millis() as i64);
    out
}
