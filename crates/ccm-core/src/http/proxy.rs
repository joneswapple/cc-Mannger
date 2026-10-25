//! `POST /v1/messages`, `POST /v1/messages/count_tokens`, `GET /v1/models`.

use std::time::Instant;

use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, HeaderName, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use reqwest::Method;
use serde::Deserialize;
use serde_json::json;

use super::{anthropic_error, header, ApiIdentity};
use crate::auth::Identity;
use crate::capture::{Captured, TeeStream};
use crate::policy::Denied;
use crate::rewrite;
use crate::stats::Event;
use crate::upstream::{passthrough_user_agent, ForwardExtra, UpstreamError};
use crate::Core;

#[derive(Deserialize)]
struct MessageRequest {
    #[serde(default)]
    model: String,
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
fn maybe_inject(core: &Core, body: Bytes) -> Bytes {
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

pub(super) async fn messages(
    State(core): State<Core>,
    ApiIdentity(id): ApiIdentity,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let parsed: MessageRequest = match serde_json::from_slice(&body) {
        Ok(p) => p,
        Err(e) => {
            return anthropic_error(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                format!("parse body: {e}"),
            )
        }
    };
    if parsed.model.is_empty() {
        return anthropic_error(
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
            "model is required",
        );
    }
    let model = parsed.model;

    // Effort tiers first, so later steps see a canonical body.
    let body = match rewrite::apply_effort(&body, &core.config.upstream.effort_levels) {
        Ok(Some(b)) => Bytes::from(b),
        Ok(None) => body,
        Err(e) => {
            return anthropic_error(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                e.to_string(),
            )
        }
    };

    match core.policy.check(id.user_id, &model) {
        Ok(Ok(())) => {}
        Ok(Err(denied)) => {
            let (status, kind) = match denied {
                Denied::RateLimited | Denied::QuotaExceeded => {
                    (StatusCode::TOO_MANY_REQUESTS, "rate_limit_error")
                }
                Denied::ModelDisabled | Denied::ModelNotAllowed => {
                    (StatusCode::FORBIDDEN, "permission_error")
                }
            };
            return anthropic_error(status, kind, denied.to_string());
        }
        Err(e) => {
            return anthropic_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "api_error",
                format!("policy: {e:#}"),
            )
        }
    }

    let body = maybe_inject(&core, body);
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

    let resp = match core
        .upstream
        .send(
            Method::POST,
            "/v1/messages",
            Some(body),
            &forward_extra(&headers),
        )
        .await
    {
        Ok((resp, account)) => (resp, account),
        Err(e) => {
            let account = e
                .downcast_ref::<UpstreamError>()
                .map_or(0, |u| u.account_id);
            let msg = format!("{e:#}");
            core.recorder
                .record(event(account, 502, &Captured::default(), msg.clone()));
            return anthropic_error(
                StatusCode::BAD_GATEWAY,
                "api_error",
                format!("upstream: {msg}"),
            );
        }
    };
    let (resp, account_id) = resp;
    let status = resp.status().as_u16();
    let head = response_head(&resp);
    let is_sse = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/event-stream"));
    let request_id_hdr = resp
        .headers()
        .get("request-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let retry_after = resp
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();

    let recorder = core.recorder.clone();
    let user = id_summary(&id);
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
        recorder.record(event(account_id, status, &c, summary));
    };

    if is_sse {
        let stream = TeeStream::new(Box::pin(resp.bytes_stream()), finish);
        return head
            .body(Body::from_stream(stream))
            .unwrap_or_else(internal);
    }
    match resp.bytes().await {
        Ok(buf) => {
            finish(Captured::from_json(&buf));
            head.body(Body::from(buf)).unwrap_or_else(internal)
        }
        Err(e) => {
            finish(Captured {
                error_message: e.to_string(),
                ..Default::default()
            });
            anthropic_error(
                StatusCode::BAD_GATEWAY,
                "api_error",
                format!("upstream: read body: {e}"),
            )
        }
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
    ApiIdentity(_): ApiIdentity,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let body = maybe_inject(&core, body);
    let resp = match core
        .upstream
        .send(
            Method::POST,
            "/v1/messages/count_tokens",
            Some(body),
            &forward_extra(&headers),
        )
        .await
    {
        Ok((resp, _)) => resp,
        Err(e) => {
            return anthropic_error(
                StatusCode::BAD_GATEWAY,
                "api_error",
                format!("upstream: {e:#}"),
            )
        }
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
            let mut row = json!({"type": "model", "id": m.id, "display_name": m.display_name});
            if let Some(c) = m.created_at {
                row["created_at"] = json!(c);
            }
            row
        })
        .collect();
    Json(json!({"data": data, "has_more": false})).into_response()
}
