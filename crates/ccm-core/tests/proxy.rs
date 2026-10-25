//! End-to-end tests for the HTTP layer against a fake Anthropic upstream.

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, Request, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::Router;
use ccm_core::accounts::Credentials;
use ccm_core::config::Config;
use ccm_core::stats::SearchFilter;
use ccm_core::Core;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

#[derive(Clone)]
struct Seen {
    path: String,
    headers: HeaderMap,
    body: Vec<u8>,
}

/// What the fake upstream answers `/v1/messages*` with.
#[derive(Clone)]
enum Reply {
    Json(u16, Vec<(&'static str, &'static str)>, &'static str),
    Sse(&'static str),
}

#[derive(Clone)]
struct Fake {
    seen: Arc<Mutex<Vec<Seen>>>,
    reply: Arc<Mutex<Reply>>,
    /// Per-account overrides, keyed by the bearer token.
    by_token: Arc<Mutex<std::collections::HashMap<String, Reply>>>,
}

async fn fake_handler(
    State(f): State<Fake>,
    uri: Uri,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    f.seen.lock().unwrap().push(Seen {
        path: uri.path().to_string(),
        headers: headers.clone(),
        body: body.to_vec(),
    });
    let bearer = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default()
        .to_string();
    if uri.path() == "/oauth/token" {
        let req: Value = serde_json::from_slice(&body).unwrap_or_default();
        let resp = match req["grant_type"].as_str() {
            Some("authorization_code") if req["code"] == "good-code" => json!({
                "access_token": "login-access", "refresh_token": "login-refresh", "expires_in": 3600,
                "scope": "user:inference user:profile",
                "account": {"email_address": "new@x.io"}, "organization": {"uuid": "org-new"}
            }),
            Some("refresh_token") => json!({
                "access_token": format!("refreshed-{}", req["refresh_token"].as_str().unwrap_or_default()),
                "expires_in": 3600
            }),
            _ => return (StatusCode::BAD_REQUEST, r#"{"error":"invalid_grant"}"#).into_response(),
        };
        return axum::Json(resp).into_response();
    }
    if uri.path() == "/api/oauth/profile" {
        // The email is derived from the token so tests can steer it.
        let email = bearer.split('-').next().unwrap_or_default();
        return axum::Json(json!({
            "account": {"email": format!("{email}@x.io"), "has_claude_max": true},
            "organization": {"uuid": format!("org-{email}")}
        }))
        .into_response();
    }
    if uri.path() == "/cloudflared-bin" {
        return FAKE_CLOUDFLARED.into_response();
    }
    if uri.path() == "/releases/latest" {
        return axum::Json(json!({
            "tag_name": "v99.1.0", "html_url": "https://example.invalid/r", "body": "notes",
            "published_at": "2026-10-01T00:00:00Z"
        }))
        .into_response();
    }
    if uri.path() == "/v1/models" {
        return (
            [("content-type", "application/json")],
            r#"{"data":[{"type":"model","id":"claude-test","display_name":"Claude Test"},{"type":"model","id":"claude-other","display_name":"Claude Other"}]}"#,
        )
            .into_response();
    }
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default()
        .to_string();
    let reply = f
        .by_token
        .lock()
        .unwrap()
        .get(&token)
        .cloned()
        .unwrap_or_else(|| f.reply.lock().unwrap().clone());
    match reply {
        Reply::Json(status, hdrs, body) => {
            let mut b = Response::builder()
                .status(status)
                .header("content-type", "application/json");
            for (k, v) in hdrs {
                b = b.header(k, v);
            }
            b.body(Body::from(body)).unwrap()
        }
        Reply::Sse(body) => Response::builder()
            .header("content-type", "text/event-stream")
            .body(Body::from(body))
            .unwrap(),
    }
}

/// Stands in for cloudflared: prints a version, its arguments, and the URL
/// line the real one would, then idles until killed.
const FAKE_CLOUDFLARED: &str = r#"#!/bin/sh
if [ "$1" = "--version" ]; then echo "cloudflared version 2026.1.0 (fake)"; exit 0; fi
echo "args: $*" >&2
if [ -n "$TUNNEL_TOKEN" ]; then
  echo 'INF Updated to new configuration config="{\"ingress\":[{\"hostname\":\"cc.example.com\"}]}"' >&2
else
  echo "INF |  https://fake-tunnel.trycloudflare.com  |" >&2
fi
exec sleep 60
"#;

struct Harness {
    core: Core,
    app: Router,
    fake: Fake,
    api_key: String,
    user_id: i64,
    _dir: tempfile::TempDir,
}

const ADMIN: &str = "admin-test-token";

async fn harness() -> Harness {
    let fake = Fake {
        seen: Default::default(),
        reply: Arc::new(Mutex::new(Reply::Json(
            200,
            vec![],
            r#"{"id":"msg_1","model":"claude-test","content":[],"usage":{"input_tokens":7,"output_tokens":3}}"#,
        ))),
        by_token: Default::default(),
    };
    let upstream = Router::new()
        .fallback(fake_handler)
        .with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });

    let dir = tempfile::tempdir().unwrap();
    let creds = dir.path().join(".credentials.json");
    ccm_core::oauth::write_credentials_file(
        &creds,
        &Credentials {
            access_token: "upstream-access".into(),
            refresh_token: "r".into(),
            expires_at: 9_999_999_999_999,
            scopes: vec![],
        },
    )
    .unwrap();
    let mut cfg = Config {
        data_dir: dir.path().join("data"),
        ..Default::default()
    };
    cfg.upstream.credentials_path = creds.to_string_lossy().into_owned();
    cfg.upstream.base_url = format!("http://{addr}");
    cfg.upstream.oauth_token_url = format!("http://{addr}/oauth/token");
    cfg.upstream.profile_url = format!("http://{addr}/api/oauth/profile");
    cfg.upstream.user_agent = "test-ua".into();
    cfg.upstream.anthropic_beta = "test-beta".into();
    cfg.update_url = format!("http://{addr}/releases/latest");
    cfg.cloudflared_download_url = format!("http://{addr}/cloudflared-bin");
    cfg.claude_settings_path = dir
        .path()
        .join("home/.claude/settings.json")
        .to_string_lossy()
        .into_owned();
    let (core, startup) = Core::open(cfg).unwrap();
    assert!(startup.imported_account.is_some());
    core.catalog.refresh().await.unwrap();
    core.admin.bootstrap(ADMIN).unwrap();
    let user_id = core.keys.create_user("alice").unwrap();
    let (api_key, _) = core.keys.issue(user_id).unwrap();
    Harness {
        app: ccm_core::http::router(core.clone()),
        core,
        fake,
        api_key,
        user_id,
        _dir: dir,
    }
}

impl Harness {
    async fn call(&self, req: Request<Body>) -> (StatusCode, HeaderMap, Vec<u8>) {
        let resp = self.app.clone().oneshot(req).await.unwrap();
        let (parts, body) = resp.into_parts();
        let bytes = body.collect().await.unwrap().to_bytes().to_vec();
        (parts.status, parts.headers, bytes)
    }

    /// A POST to `path` authenticated with the harness user's key.
    fn post(&self, path: &str) -> axum::http::request::Builder {
        Request::post(path)
            .header("x-api-key", &self.api_key)
            .header("content-type", "application/json")
    }

    async fn messages(
        &self,
        body: &'static str,
        extra: &[(&str, &str)],
    ) -> (StatusCode, HeaderMap, Vec<u8>) {
        let mut b = self.post("/v1/messages");
        for (k, v) in extra {
            b = b.header(*k, *v);
        }
        self.call(b.body(Body::from(body)).unwrap()).await
    }

    fn last_seen(&self, path: &str) -> Seen {
        self.fake
            .seen
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|s| s.path == path)
            .cloned()
            .unwrap_or_else(|| panic!("upstream never saw {path}"))
    }

    fn set_reply(&self, r: Reply) {
        *self.fake.reply.lock().unwrap() = r;
    }

    fn reply_for(&self, token: &str, r: Reply) {
        self.fake.by_token.lock().unwrap().insert(token.into(), r);
    }

    /// Adds an upstream account authenticating with `token`.
    fn add_account(&self, label: &str, token: &str, priority: i64, group: &str) -> i64 {
        self.core
            .accounts
            .create(
                ccm_core::accounts::Account {
                    label: label.into(),
                    enabled: true,
                    priority,
                    account_group: group.into(),
                    ..Default::default()
                },
                &Credentials {
                    access_token: token.into(),
                    ..Default::default()
                },
            )
            .unwrap()
    }

    /// Bearer tokens the upstream saw on `/v1/messages`, in order.
    fn tokens_seen(&self) -> Vec<String> {
        self.fake
            .seen
            .lock()
            .unwrap()
            .iter()
            .filter(|s| s.path == "/v1/messages")
            .map(|s| {
                hdr(&s.headers, "authorization")
                    .trim_start_matches("Bearer ")
                    .to_string()
            })
            .collect()
    }

    async fn admin(&self, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        let req = Request::builder()
            .method(method)
            .uri(path)
            .header("authorization", format!("Bearer {ADMIN}"))
            .header("content-type", "application/json")
            .body(body.map_or(Body::empty(), |b| Body::from(b.to_string())))
            .unwrap();
        let (status, _, bytes) = self.call(req).await;
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }
}

fn json(b: &[u8]) -> Value {
    serde_json::from_slice(b).unwrap_or_else(|_| panic!("not json: {}", String::from_utf8_lossy(b)))
}

fn hdr<'a>(h: &'a HeaderMap, k: &str) -> &'a str {
    h.get(k).and_then(|v| v.to_str().ok()).unwrap_or("")
}

#[tokio::test]
async fn forwards_with_account_token_and_records_usage() {
    let h = harness().await;
    let (status, _, body) = h
        .messages(
            r#"{"model":"claude-test","messages":[{"role":"user","content":"hi"}]}"#,
            &[],
        )
        .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    assert_eq!(json(&body)["id"], "msg_1");
    let seen = h.last_seen("/v1/messages");
    assert_eq!(
        hdr(&seen.headers, "authorization"),
        "Bearer upstream-access"
    );
    assert_eq!(hdr(&seen.headers, "anthropic-version"), "2023-06-01");
    h.core.recorder.flush();
    let (rows, _) = h.core.recorder.search(&SearchFilter::default()).unwrap();
    assert_eq!(
        (rows[0].input_tokens, rows[0].output_tokens, rows[0].status),
        (7, 3, 200)
    );
    assert_eq!(rows[0].user_id, h.user_id);
    assert!(rows[0].account_id.is_some());
}

#[tokio::test]
async fn merges_beta_and_passes_claude_user_agent() {
    let h = harness().await;
    h.messages(
        r#"{"model":"claude-test","messages":[]}"#,
        &[
            (
                "anthropic-beta",
                "interleaved-thinking-2025-05-14,test-beta",
            ),
            ("user-agent", "claude-cli/2.0.0 (linux; x86_64)"),
        ],
    )
    .await;
    let seen = h.last_seen("/v1/messages");
    assert_eq!(
        hdr(&seen.headers, "anthropic-beta"),
        "test-beta,interleaved-thinking-2025-05-14"
    );
    assert_eq!(
        hdr(&seen.headers, "user-agent"),
        "claude-cli/2.0.0 (linux; x86_64)"
    );
}

#[tokio::test]
async fn replaces_unknown_user_agent() {
    let h = harness().await;
    h.messages(
        r#"{"model":"claude-test","messages":[]}"#,
        &[("user-agent", "curl/8.4.0")],
    )
    .await;
    assert_eq!(
        hdr(&h.last_seen("/v1/messages").headers, "user-agent"),
        "test-ua"
    );
}

#[tokio::test]
async fn injects_system_once() {
    let h = harness().await;
    h.messages(
        r#"{"model":"claude-test","system":"Be terse.","messages":[]}"#,
        &[],
    )
    .await;
    let sent = json(&h.last_seen("/v1/messages").body);
    assert_eq!(
        sent["system"][0]["text"],
        ccm_core::rewrite::CLAUDE_CODE_IDENTIFIER
    );
    assert_eq!(sent["system"][1]["text"], "Be terse.");

    let from_claude = r#"{"model":"claude-test","system":[{"type":"text","text":"You are Claude Code, Anthropic's official CLI for Claude."}],"messages":[]}"#;
    h.messages(from_claude, &[]).await;
    assert_eq!(
        h.last_seen("/v1/messages").body,
        from_claude.as_bytes(),
        "body must pass through byte-for-byte"
    );
}

#[tokio::test]
async fn expands_effort_and_rejects_unknown() {
    let h = harness().await;
    h.messages(
        r#"{"model":"claude-test","effort":"high","messages":[]}"#,
        &[],
    )
    .await;
    let sent = json(&h.last_seen("/v1/messages").body);
    assert_eq!(sent["max_tokens"], 32768);
    assert_eq!(sent["thinking"]["budget_tokens"], 15000);
    assert!(sent.get("effort").is_none());

    let (status, _, body) = h
        .messages(r#"{"model":"claude-test","effort":"turbo"}"#, &[])
        .await;
    assert_eq!(status, 400);
    assert_eq!(json(&body)["error"]["type"], "invalid_request_error");
}

#[tokio::test]
async fn forwards_upstream_rate_limit_verbatim() {
    let h = harness().await;
    let upstream_body = r#"{"type":"error","error":{"type":"rate_limit_error","message":"Error"},"request_id":"req_011CaczwhMBhKT7rhhGTWxwu"}"#;
    h.set_reply(Reply::Json(429, vec![("retry-after", "30")], upstream_body));
    let (status, headers, body) = h
        .messages(r#"{"model":"claude-test","messages":[]}"#, &[])
        .await;
    assert_eq!(status, 429);
    assert_eq!(hdr(&headers, "retry-after"), "30");
    assert_eq!(body, upstream_body.as_bytes());
    h.core.recorder.flush();
    let (rows, _) = h
        .core
        .recorder
        .search(&SearchFilter {
            status: "429".into(),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        rows[0].error,
        "rate_limit_error: Error (req_id=req_011CaczwhMBhKT7rhhGTWxwu)"
    );
}

#[tokio::test]
async fn streams_sse_and_captures_usage() {
    let h = harness().await;
    let sse = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"m1\",\"usage\":{\"input_tokens\":10,\"cache_read_input_tokens\":2}}}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"usage\":{\"input_tokens\":10,\"output_tokens\":5}}\n\n",
    );
    h.set_reply(Reply::Sse(sse));
    let (status, headers, body) = h
        .messages(r#"{"model":"claude-test","stream":true}"#, &[])
        .await;
    assert_eq!(status, 200);
    assert!(hdr(&headers, "content-type").starts_with("text/event-stream"));
    assert_eq!(body, sse.as_bytes());
    h.core.recorder.flush();
    let (rows, _) = h.core.recorder.search(&SearchFilter::default()).unwrap();
    assert_eq!(
        (
            rows[0].input_tokens,
            rows[0].output_tokens,
            rows[0].cache_read_tokens
        ),
        (10, 5, 2)
    );
}

#[tokio::test]
async fn rejects_bad_keys_with_anthropic_errors() {
    let h = harness().await;
    for (key, hint) in [
        ("sk-cp-bogus", "no matching key"),
        ("sk-ant-abc", "real Anthropic"),
        ("0123456789abcdef0123", "invite token"),
    ] {
        let req = Request::post("/v1/messages")
            .header("x-api-key", key)
            .body(Body::from("{}"))
            .unwrap();
        let (status, _, body) = h.call(req).await;
        assert_eq!(status, 401);
        let v = json(&body);
        assert_eq!(v["type"], "error");
        assert_eq!(v["error"]["type"], "authentication_error");
        assert!(
            v["error"]["message"].as_str().unwrap().contains(hint),
            "{v}"
        );
    }
    let (status, _, _) = h
        .call(
            Request::post("/v1/messages/count_tokens")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await;
    assert_eq!(status, 401);
    let (status, _, _) = h
        .call(Request::get("/v1/models").body(Body::empty()).unwrap())
        .await;
    assert_eq!(status, 401);
}

#[tokio::test]
async fn policy_denials() {
    let h = harness().await;
    h.core
        .policy
        .set_disabled_models(&["claude-test".into()])
        .unwrap();
    let (status, _, body) = h
        .messages(r#"{"model":"claude-test","messages":[]}"#, &[])
        .await;
    assert_eq!(status, 403);
    assert_eq!(json(&body)["error"]["type"], "permission_error");
    let (status, _, _) = h.messages(r#"{"messages":[]}"#, &[]).await;
    assert_eq!(status, 400);
}

#[tokio::test]
async fn count_tokens_proxies_and_injects() {
    let h = harness().await;
    h.set_reply(Reply::Json(200, vec![], r#"{"input_tokens":42}"#));
    let body = r#"{"model":"claude-test","messages":[{"role":"user","content":"hi"}]}"#;
    let (status, _, out) = h
        .call(
            h.post("/v1/messages/count_tokens")
                .body(Body::from(body))
                .unwrap(),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(json(&out)["input_tokens"], 42);
    let sent = String::from_utf8(h.last_seen("/v1/messages/count_tokens").body).unwrap();
    assert!(sent.contains("You are Claude Code"));
    h.core.recorder.flush();
    assert_eq!(
        h.core.recorder.search(&SearchFilter::default()).unwrap().1,
        0,
        "count_tokens is not recorded"
    );
}

#[tokio::test]
async fn models_list_from_catalog() {
    let h = harness().await;
    let req = Request::get("/v1/models")
        .header("x-api-key", &h.api_key)
        .body(Body::empty())
        .unwrap();
    let (status, _, body) = h.call(req).await;
    assert_eq!(status, 200);
    let v = json(&body);
    assert_eq!(v["has_more"], false);
    let ids: Vec<_> = v["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(ids, vec!["claude-other", "claude-test"]);
}

#[tokio::test]
async fn admin_api_requires_token() {
    let h = harness().await;
    for token in [None, Some("wrong")] {
        let mut b = Request::get("/admin/api/users");
        if let Some(t) = token {
            b = b.header("x-admin-token", t);
        }
        let (status, _, body) = h.call(b.body(Body::empty()).unwrap()).await;
        assert_eq!(status, 401);
        assert_eq!(json(&body)["error"], "unauthorized");
    }
    let req = Request::get("/admin/api/status")
        .header("cookie", format!("other=1; ccm_admin={ADMIN}"))
        .body(Body::empty())
        .unwrap();
    assert_eq!(h.call(req).await.0, 200);
    // Healthz stays public but reveals nothing about accounts.
    let (status, _, body) = h
        .call(Request::get("/healthz").body(Body::empty()).unwrap())
        .await;
    assert_eq!(status, 200);
    assert!(json(&body).get("upstream").is_none());
}

#[tokio::test]
async fn register_flow_via_admin_invite() {
    let h = harness().await;
    let (status, v) = h
        .admin(
            "POST",
            "/admin/api/invites",
            Some(json!({"label": "bob-laptop", "ttl_hours": 1})),
        )
        .await;
    assert_eq!(status, 200);
    let token = v["token"].as_str().unwrap().to_string();
    let req = Request::post("/register")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"token": token, "label": "bob"}).to_string(),
        ))
        .unwrap();
    let (status, _, body) = h.call(req).await;
    assert_eq!(status, 200);
    let key = json(&body)["api_key"].as_str().unwrap().to_string();
    let req = Request::get("/v1/models")
        .header("x-api-key", &key)
        .body(Body::empty())
        .unwrap();
    assert_eq!(h.call(req).await.0, 200);
    // Single use.
    let req = Request::post("/register")
        .header("content-type", "application/json")
        .body(Body::from(json!({"token": token}).to_string()))
        .unwrap();
    let (status, _, body) = h.call(req).await;
    assert_eq!(status, 400);
    assert_eq!(json(&body)["error"], "invite already redeemed");
}

#[tokio::test]
async fn admin_user_key_policy_lifecycle() {
    let h = harness().await;
    let (_, v) = h.admin("GET", "/admin/api/users", None).await;
    assert_eq!(v["users"][0]["label"], "alice");
    assert!(v["users"][0]["keys"][0].get("hash").is_none());

    let path = format!("/admin/api/users/{}/policy", h.user_id);
    let (status, _) = h
        .admin(
            "POST",
            &path,
            Some(json!({"allowed_models": ["claude-other"], "rpm_limit": 5})),
        )
        .await;
    assert_eq!(status, 204);
    let (_, p) = h.admin("GET", &path, None).await;
    assert_eq!(p["allowed_models"], json!(["claude-other"]));
    let (status, _, _) = h
        .messages(r#"{"model":"claude-test","messages":[]}"#, &[])
        .await;
    assert_eq!(status, 403);

    let (status, _) = h
        .admin(
            "POST",
            &format!("/admin/api/users/{}/disable", h.user_id),
            None,
        )
        .await;
    assert_eq!(status, 204);
    let (status, _, _) = h
        .messages(r#"{"model":"claude-other","messages":[]}"#, &[])
        .await;
    assert_eq!(status, 403);
    let (status, _) = h.admin("POST", "/admin/api/users/9999/disable", None).await;
    assert_eq!(status, 404);

    let (_, issued) = h
        .admin(
            "POST",
            "/admin/api/keys/issue",
            Some(json!({"user_id": h.user_id})),
        )
        .await;
    assert!(issued["api_key"].as_str().unwrap().starts_with("sk-cp-"));
    let (status, _) = h
        .admin(
            "POST",
            &format!("/admin/api/keys/{}/revoke", issued["key_id"]),
            None,
        )
        .await;
    assert_eq!(status, 204);
}

#[tokio::test]
async fn admin_settings_stats_usage_accounts() {
    let h = harness().await;
    h.messages(r#"{"model":"claude-test","messages":[]}"#, &[])
        .await;
    h.core.recorder.flush();

    let (_, s) = h.admin("GET", "/admin/api/settings", None).await;
    assert_eq!(s["upstream"]["anthropic_beta"], "test-beta");
    assert_eq!(s["models"].as_array().unwrap().len(), 2);
    let (status, _) = h
        .admin(
            "POST",
            "/admin/api/settings/disabled-models",
            Some(json!({"models": ["claude-other"]})),
        )
        .await;
    assert_eq!(status, 204);
    let (_, r) = h
        .admin("POST", "/admin/api/settings/refresh-models", None)
        .await;
    assert_eq!(r["models"], json!(["claude-other", "claude-test"]));

    let (_, st) = h.admin("GET", "/admin/api/stats?days=7", None).await;
    assert_eq!(st["by_model"][0]["model"], "claude-test");
    assert_eq!(st["by_account"][0]["requests"], 1);
    let (_, u) = h
        .admin(
            "GET",
            "/admin/api/usage?model=test&status=ok&page_size=10",
            None,
        )
        .await;
    assert_eq!(u["total"], 1);
    assert_eq!(u["events"][0]["user_label"], "alice");

    let (_, a) = h.admin("GET", "/admin/api/accounts", None).await;
    let acct = &a["accounts"][0];
    assert_eq!(acct["label"], "claude-cli");
    assert!(
        !a.to_string().contains("upstream-access"),
        "tokens must never be listed"
    );
}

const RATE_LIMITED: &str = r#"{"type":"error","error":{"type":"rate_limit_error","message":"Error"},"request_id":"req_a"}"#;

#[tokio::test]
async fn fails_over_on_429_and_cools_the_account_down() {
    let h = harness().await;
    let primary = h.add_account("primary", "primary-tok", 10, "");
    h.reply_for(
        "primary-tok",
        Reply::Json(429, vec![("retry-after", "120")], RATE_LIMITED),
    );

    let (status, _, body) = h
        .messages(r#"{"model":"claude-test","messages":[]}"#, &[])
        .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    assert_eq!(h.tokens_seen(), vec!["primary-tok", "upstream-access"]);

    let a = h.core.accounts.get(primary).unwrap();
    let left = a.cooldown_left(ccm_core_now());
    assert!((110..=120).contains(&left), "cooldown left {left}");

    // While cooling, the primary is skipped without being called.
    h.messages(r#"{"model":"claude-test","messages":[]}"#, &[])
        .await;
    assert_eq!(h.tokens_seen().len(), 3);
    assert_eq!(h.tokens_seen()[2], "upstream-access");

    h.core.recorder.flush();
    let (rows, _) = h
        .core
        .recorder
        .search(&SearchFilter {
            status: "429".into(),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].account_id, Some(primary));
    assert!(
        rows[0].error.starts_with("failover: upstream 429"),
        "{}",
        rows[0].error
    );
}

#[tokio::test]
async fn all_accounts_limited_forwards_last_error_then_short_circuits() {
    let h = harness().await;
    h.add_account("second", "second-tok", 10, "");
    h.set_reply(Reply::Json(429, vec![("retry-after", "90")], RATE_LIMITED));

    let (status, headers, body) = h
        .messages(r#"{"model":"claude-test","messages":[]}"#, &[])
        .await;
    assert_eq!(status, 429);
    assert_eq!(
        body,
        RATE_LIMITED.as_bytes(),
        "the upstream's own error is forwarded"
    );
    assert_eq!(hdr(&headers, "retry-after"), "90");
    assert_eq!(h.tokens_seen().len(), 2);

    // Both accounts are now parked: answered locally, upstream not called.
    let (status, headers, body) = h
        .messages(r#"{"model":"claude-test","messages":[]}"#, &[])
        .await;
    assert_eq!(status, 429);
    assert_eq!(json(&body)["error"]["type"], "rate_limit_error");
    let retry: i64 = hdr(&headers, "retry-after").parse().unwrap();
    assert!((80..=90).contains(&retry), "{retry}");
    assert_eq!(h.tokens_seen().len(), 2);
}

#[tokio::test]
async fn server_errors_fail_over_without_cooldown() {
    let h = harness().await;
    let primary = h.add_account("primary", "primary-tok", 10, "");
    h.reply_for(
        "primary-tok",
        Reply::Json(
            529,
            vec![],
            r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
        ),
    );
    let (status, _, _) = h
        .messages(r#"{"model":"claude-test","messages":[]}"#, &[])
        .await;
    assert_eq!(status, 200);
    assert_eq!(h.core.accounts.get(primary).unwrap().cooldown_until, 0);
}

#[tokio::test]
async fn request_errors_are_not_retried() {
    let h = harness().await;
    h.add_account("primary", "primary-tok", 10, "");
    let bad = r#"{"type":"error","error":{"type":"invalid_request_error","message":"max_tokens: required"}}"#;
    h.reply_for("primary-tok", Reply::Json(400, vec![], bad));
    let (status, _, body) = h
        .messages(r#"{"model":"claude-test","messages":[]}"#, &[])
        .await;
    assert_eq!(status, 400);
    assert_eq!(body, bad.as_bytes());
    assert_eq!(h.tokens_seen(), vec!["primary-tok"]);
}

#[tokio::test]
async fn unauthorized_account_without_refresh_token_needs_reauth() {
    let h = harness().await;
    let primary = h.add_account("primary", "primary-tok", 10, "");
    h.reply_for(
        "primary-tok",
        Reply::Json(
            401,
            vec![],
            r#"{"type":"error","error":{"type":"authentication_error","message":"invalid"}}"#,
        ),
    );
    let (status, _, _) = h
        .messages(r#"{"model":"claude-test","messages":[]}"#, &[])
        .await;
    assert_eq!(status, 200);
    assert_eq!(h.core.accounts.get(primary).unwrap().status, "needs_reauth");
    h.messages(r#"{"model":"claude-test","messages":[]}"#, &[])
        .await;
    assert_eq!(
        h.tokens_seen(),
        vec!["primary-tok", "upstream-access", "upstream-access"]
    );
}

#[tokio::test]
async fn sse_after_failover_streams_from_the_next_account() {
    let h = harness().await;
    h.add_account("primary", "primary-tok", 10, "");
    h.reply_for("primary-tok", Reply::Json(429, vec![], RATE_LIMITED));
    let sse = "event: message_delta\ndata: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":5}}\n\n";
    h.set_reply(Reply::Sse(sse));
    let (status, headers, body) = h
        .messages(r#"{"model":"claude-test","stream":true}"#, &[])
        .await;
    assert_eq!(status, 200);
    assert!(hdr(&headers, "content-type").starts_with("text/event-stream"));
    assert_eq!(body, sse.as_bytes());
}

#[tokio::test]
async fn sticky_sessions_and_account_groups() {
    let h = harness().await;
    let team = h.add_account("team", "team-tok", 0, "team");
    h.add_account("top", "top-tok", 10, "");

    // Unrestricted users get the highest-priority healthy account.
    h.messages(r#"{"model":"claude-test","messages":[]}"#, &[])
        .await;
    assert_eq!(h.tokens_seen().last().unwrap(), "top-tok");

    // A user bound to the "team" group only ever uses team accounts.
    h.core
        .policy
        .set_policy(
            h.user_id,
            &ccm_core::policy::Policy {
                account_group: Some("team".into()),
                ..Default::default()
            },
        )
        .unwrap();
    h.messages(
        r#"{"model":"claude-test","metadata":{"user_id":"sess-1"},"messages":[]}"#,
        &[],
    )
    .await;
    assert_eq!(h.tokens_seen().last().unwrap(), "team-tok");

    // With the group's only account cooling, the user is told to wait
    // rather than spilling onto other groups' accounts.
    h.core.pool.cool_down(team, 60, "test");
    let (status, _, _) = h
        .messages(r#"{"model":"claude-test","messages":[]}"#, &[])
        .await;
    assert_eq!(status, 429);
}

#[tokio::test]
async fn admin_can_update_accounts() {
    let h = harness().await;
    let id = h.add_account("x", "x-tok", 0, "");
    h.core.pool.cool_down(id, 600, "test");
    let (status, a) = h
        .admin(
            "POST",
            &format!("/admin/api/accounts/{id}"),
            Some(json!({"priority": 7, "weight": 3, "account_group": "g", "clear_cooldown": true})),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        (
            a["priority"].as_i64(),
            a["weight"].as_i64(),
            a["cooldown_until"].as_i64()
        ),
        (Some(7), Some(3), Some(0))
    );
    assert_eq!(a["account_group"], "g");
    assert_eq!(
        a["status_reason"], "",
        "clearing a cooldown clears its reason"
    );
    let (status, _) = h
        .admin(
            "POST",
            "/admin/api/accounts/9999",
            Some(json!({"priority": 1})),
        )
        .await;
    assert_eq!(status, 404);
    let (_, list) = h.admin("GET", "/admin/api/accounts", None).await;
    assert_eq!(list["pool"]["strategy"], "priority");
    assert!(list["now"].as_i64().unwrap() > 0);
}

fn ccm_core_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

#[tokio::test]
async fn oauth_login_adds_then_updates_account() {
    let h = harness().await;
    let (status, start) = h
        .admin(
            "POST",
            "/admin/api/accounts/oauth/start",
            Some(json!({"label": "via-login", "account_group": "g1"})),
        )
        .await;
    assert_eq!(status, 200);
    let url = start["url"].as_str().unwrap();
    assert!(
        url.starts_with("https://claude.ai/oauth/authorize?"),
        "{url}"
    );
    assert!(url.contains("code_challenge_method=S256"));
    let state = start["state"].as_str().unwrap();

    let (status, a) = h
        .admin(
            "POST",
            "/admin/api/accounts/oauth/complete",
            Some(json!({"code": format!("good-code#{state}")})),
        )
        .await;
    assert_eq!(status, 200, "{a}");
    assert_eq!(
        (
            a["label"].as_str(),
            a["email"].as_str(),
            a["account_group"].as_str()
        ),
        (Some("via-login"), Some("new@x.io"), Some("g1"))
    );
    let id = a["id"].as_i64().unwrap();
    assert_eq!(
        h.core.accounts.load_credentials(id).unwrap().access_token,
        "login-access"
    );

    // Replaying the state fails; a fresh login for the same email updates
    // the account instead of adding one.
    let (status, _) = h
        .admin(
            "POST",
            "/admin/api/accounts/oauth/complete",
            Some(json!({"code": format!("good-code#{state}")})),
        )
        .await;
    assert_eq!(status, 400);
    h.core.accounts.set_status(id, "needs_reauth", "x").unwrap();
    let (_, start2) = h
        .admin("POST", "/admin/api/accounts/oauth/start", None)
        .await;
    let (_, a2) = h
        .admin(
            "POST",
            "/admin/api/accounts/oauth/complete",
            Some(json!({"code": "good-code", "state": start2["state"]})),
        )
        .await;
    assert_eq!(a2["id"].as_i64(), Some(id));
    assert_eq!(a2["status"], "active");
    assert_eq!(h.core.accounts.count().unwrap(), 2);

    let (status, _) = h
        .admin(
            "POST",
            "/admin/api/accounts/oauth/complete",
            Some(json!({"code": "bad-code#nope"})),
        )
        .await;
    assert_eq!(status, 400);
}

#[tokio::test]
async fn oauth_callback_completes_login_publicly() {
    let h = harness().await;
    let start = h.core.login.start(Default::default());
    let req = Request::get(format!(
        "/oauth/callback?code=good-code&state={}",
        start.state
    ))
    .body(Body::empty())
    .unwrap();
    let (status, headers, body) = h.call(req).await;
    assert_eq!(status, 200);
    assert!(hdr(&headers, "content-type").starts_with("text/html"));
    assert!(String::from_utf8_lossy(&body).contains("new@x.io"));
    let req = Request::get("/oauth/callback?code=good-code&state=forged")
        .body(Body::empty())
        .unwrap();
    assert_eq!(h.call(req).await.0, 400);
}

#[tokio::test]
async fn import_single_batch_and_dedup_by_email() {
    let h = harness().await;
    let doc = json!({"claudeAiOauth": {"accessToken": "bob-1", "refreshToken": "r", "expiresAt": 9_999_999_999_999i64}});
    let (status, r) = h
        .admin(
            "POST",
            "/admin/api/accounts/import",
            Some(json!({"credentials": doc, "label": "bob"})),
        )
        .await;
    assert_eq!(status, 200, "{r}");
    let bob = &r["imported"][0];
    assert_eq!(
        (
            bob["label"].as_str(),
            bob["email"].as_str(),
            bob["plan"].as_str()
        ),
        (Some("bob"), Some("bob@x.io"), Some("claude_max"))
    );

    let batch = json!([
        {"access_token": "carol-1", "refresh_token": "r", "expires_in": 3600},
        {"label": "bob again", "credentials": {"accessToken": "bob-2"}},
        {"nonsense": true}
    ]);
    let (status, r) = h
        .admin(
            "POST",
            "/admin/api/accounts/import",
            Some(json!({"credentials": batch})),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(r["imported"].as_array().unwrap().len(), 2);
    assert_eq!(r["errors"][0]["index"], 2);
    // bob's second import updated the existing account.
    assert_eq!(r["imported"][1]["id"], bob["id"]);
    let bob_id = bob["id"].as_i64().unwrap();
    assert_eq!(
        h.core
            .accounts
            .load_credentials(bob_id)
            .unwrap()
            .access_token,
        "bob-2"
    );
    assert_eq!(h.core.accounts.count().unwrap(), 3);

    let (status, _) = h
        .admin(
            "POST",
            "/admin/api/accounts/import",
            Some(json!({"credentials": {"x": 1}})),
        )
        .await;
    assert_eq!(status, 400);
}

#[tokio::test]
async fn refresh_pin_proxy_and_delete() {
    let h = harness().await;
    let id = h.add_account("ops", "ops-tok", 0, "");
    h.core
        .accounts
        .save_credentials(
            id,
            &Credentials {
                access_token: "ops-tok".into(),
                refresh_token: "ops-ref".into(),
                expires_at: 9_999_999_999_999,
                scopes: vec![],
            },
        )
        .unwrap();

    let (status, a) = h
        .admin("POST", &format!("/admin/api/accounts/{id}/refresh"), None)
        .await;
    assert_eq!(status, 200, "{a}");
    assert_eq!(
        h.core.accounts.load_credentials(id).unwrap().access_token,
        "refreshed-ops-ref"
    );
    // The profile is re-read with the new token.
    assert_eq!(a["email"], "refreshed@x.io");
    // Upstream calls use the refreshed token right away.
    let (_, pinned) = h
        .admin("POST", &format!("/admin/api/accounts/{id}/pin"), None)
        .await;
    assert!(pinned["priority"].as_i64().unwrap() > 0);
    h.messages(r#"{"model":"claude-test","messages":[]}"#, &[])
        .await;
    assert_eq!(h.tokens_seen().last().unwrap(), "refreshed-ops-ref");

    let (status, e) = h
        .admin(
            "POST",
            &format!("/admin/api/accounts/{id}"),
            Some(json!({"proxy_url": "ftp://x"})),
        )
        .await;
    assert_eq!(status, 400, "{e}");
    let (status, a) = h
        .admin(
            "POST",
            &format!("/admin/api/accounts/{id}"),
            Some(json!({"proxy_url": "socks5://127.0.0.1:1"})),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(a["proxy_url"], "socks5://127.0.0.1:1");
    // A dead proxy fails that account over; it never goes direct.
    let (status, _, _) = h
        .messages(r#"{"model":"claude-test","messages":[]}"#, &[])
        .await;
    assert_eq!(status, 200);
    assert_eq!(h.tokens_seen().last().unwrap(), "upstream-access");

    let (status, _) = h
        .admin("POST", &format!("/admin/api/accounts/{id}/delete"), None)
        .await;
    assert_eq!(status, 204);
    assert_eq!(
        h.admin("POST", &format!("/admin/api/accounts/{id}/delete"), None)
            .await
            .0,
        404
    );
}

#[tokio::test]
async fn local_bridge_needs_no_token_but_only_reaches_admin_api() {
    let h = harness().await;
    let (status, body) = ccm_core::http::call_local(&h.app, "GET", "/admin/api/status", None)
        .await
        .unwrap();
    assert_eq!(status, 200);
    assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
    let (status, _) = ccm_core::http::call_local(
        &h.app,
        "POST",
        "/admin/api/users",
        Some(json!({"label": "x"})),
    )
    .await
    .unwrap();
    assert_eq!(status, 200);
    assert!(
        ccm_core::http::call_local(&h.app, "POST", "/v1/messages", None)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn captures_quota_headers_and_routes_around_exhausted_accounts() {
    let h = harness().await;
    let busy = h.add_account("busy", "busy-tok", 10, "");
    h.reply_for(
        "busy-tok",
        Reply::Json(
            200,
            vec![
                ("anthropic-ratelimit-unified-5h-utilization", "0.97"),
                ("anthropic-ratelimit-unified-5h-reset", "9999999999"),
                ("anthropic-ratelimit-unified-7d-utilization", "0.40"),
                ("anthropic-ratelimit-unified-status", "allowed_warning"),
            ],
            r#"{"id":"m","usage":{"input_tokens":1,"output_tokens":1}}"#,
        ),
    );
    h.messages(r#"{"model":"claude-test","messages":[]}"#, &[])
        .await;
    assert_eq!(h.tokens_seen().last().unwrap(), "busy-tok");

    let (_, list) = h.admin("GET", "/admin/api/accounts", None).await;
    let acct = list["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == busy)
        .unwrap()
        .clone();
    assert_eq!(acct["quota"]["five_hour_utilization"], 0.97);
    assert_eq!(acct["quota"]["seven_day_utilization"], 0.4);
    assert_eq!(acct["quota"]["status"], "allowed_warning");
    assert_ne!(
        list["recommended"], busy,
        "an account over the soft limit isn't recommended"
    );

    // Next request goes to the account with headroom.
    h.messages(r#"{"model":"claude-test","messages":[]}"#, &[])
        .await;
    assert_eq!(h.tokens_seen().last().unwrap(), "upstream-access");
}

#[tokio::test]
async fn model_routes_rewrite_before_policy_and_upstream() {
    let h = harness().await;
    let (status, _) = h
        .admin(
            "POST",
            "/admin/api/settings/model-routes",
            Some(json!({"routes": [{"pattern": "re:^gpt-", "target": "claude-test"}]})),
        )
        .await;
    assert_eq!(status, 204);
    let (status, e) = h
        .admin(
            "POST",
            "/admin/api/settings/model-routes",
            Some(json!({"routes": [{"pattern": "re:(", "target": "x"}]})),
        )
        .await;
    assert_eq!(status, 400, "{e}");
    let (_, settings) = h.admin("GET", "/admin/api/settings", None).await;
    assert_eq!(settings["model_routes"][0]["target"], "claude-test");

    h.messages(r#"{"model":"gpt-4o","messages":[]}"#, &[]).await;
    assert_eq!(
        json(&h.last_seen("/v1/messages").body)["model"],
        "claude-test"
    );
    h.core.recorder.flush();
    let (rows, _) = h.core.recorder.search(&SearchFilter::default()).unwrap();
    assert_eq!(
        rows[0].model, "claude-test",
        "stats record the routed model"
    );
    // Policy applies to the routed model.
    h.core
        .policy
        .set_disabled_models(&["claude-test".into()])
        .unwrap();
    let (status, _, _) = h.messages(r#"{"model":"gpt-4o","messages":[]}"#, &[]).await;
    assert_eq!(status, 403);
}

#[tokio::test]
async fn openai_chat_completions_non_streaming() {
    let h = harness().await;
    h.set_reply(Reply::Json(
        200,
        vec![],
        r#"{"id":"msg_7","type":"message","stop_reason":"end_turn","content":[{"type":"text","text":"Hi there"}],"usage":{"input_tokens":9,"output_tokens":2}}"#,
    ));
    let req = json!({"model": "claude-test", "messages": [{"role": "system", "content": "sys"}, {"role": "user", "content": "hello"}], "max_tokens": 50});
    let (status, _, body) = h
        .call(
            Request::post("/v1/chat/completions")
                .header("authorization", format!("Bearer {}", h.api_key))
                .header("content-type", "application/json")
                .body(Body::from(req.to_string()))
                .unwrap(),
        )
        .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let v = json(&body);
    assert_eq!(v["object"], "chat.completion");
    assert_eq!(v["choices"][0]["message"]["content"], "Hi there");
    assert_eq!(v["choices"][0]["finish_reason"], "stop");
    assert_eq!(v["usage"]["total_tokens"], 11);

    let sent = json(&h.last_seen("/v1/messages").body);
    assert_eq!(sent["max_tokens"], 50);
    assert_eq!(sent["messages"][0]["content"][0]["text"], "hello");
    // The Claude Code identifier comes first, then the OpenAI system prompt.
    assert_eq!(
        sent["system"][0]["text"],
        ccm_core::rewrite::CLAUDE_CODE_IDENTIFIER
    );
    assert_eq!(sent["system"][1]["text"], "sys");
    h.core.recorder.flush();
    let (rows, _) = h.core.recorder.search(&SearchFilter::default()).unwrap();
    assert_eq!((rows[0].input_tokens, rows[0].output_tokens), (9, 2));
}

#[tokio::test]
async fn openai_chat_completions_streaming_and_errors() {
    let h = harness().await;
    let sse = concat!(
        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_s\",\"usage\":{\"input_tokens\":3}}}\n\n",
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Yo\"}}\n\n",
        "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\n",
        "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
    );
    h.set_reply(Reply::Sse(sse));
    let req = json!({"model": "claude-test", "messages": [{"role": "user", "content": "hi"}], "stream": true});
    let call = |body: Value| {
        Request::post("/v1/chat/completions")
            .header("authorization", format!("Bearer {}", h.api_key))
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    };
    let (status, headers, body) = h.call(call(req)).await;
    assert_eq!(status, 200);
    assert!(hdr(&headers, "content-type").starts_with("text/event-stream"));
    let text = String::from_utf8(body).unwrap();
    assert!(text.contains("\"content\":\"Yo\""), "{text}");
    assert!(text.contains("\"finish_reason\":\"stop\""));
    assert!(text.trim_end().ends_with("data: [DONE]"));
    h.core.recorder.flush();
    let (rows, _) = h.core.recorder.search(&SearchFilter::default()).unwrap();
    assert_eq!(
        (rows[0].input_tokens, rows[0].output_tokens),
        (3, 1),
        "usage still captured under translation"
    );

    // Upstream errors keep their status and an error.message body.
    h.set_reply(Reply::Json(429, vec![("retry-after", "5")], RATE_LIMITED));
    let (status, headers, body) = h
        .call(call(
            json!({"model": "claude-test", "messages": [{"role": "user", "content": "hi"}]}),
        ))
        .await;
    assert_eq!(status, 429);
    assert_eq!(hdr(&headers, "retry-after"), "5");
    assert_eq!(json(&body)["error"]["message"], "Error");

    let (status, _, body) = h.call(call(json!({"model": "claude-test"}))).await;
    assert_eq!(status, 400);
    assert!(json(&body)["error"]["message"]
        .as_str()
        .unwrap()
        .contains("messages"));
}

#[tokio::test]
async fn models_list_has_openai_fields() {
    let h = harness().await;
    let req = Request::get("/v1/models")
        .header("authorization", format!("Bearer {}", h.api_key))
        .body(Body::empty())
        .unwrap();
    let v = json(&h.call(req).await.2);
    assert_eq!(v["object"], "list");
    assert_eq!(v["data"][0]["object"], "model");
    assert_eq!(v["data"][0]["owned_by"], "anthropic");
    assert_eq!(v["data"][0]["type"], "model");
}

#[tokio::test]
async fn metrics_require_admin_and_report_counters() {
    let h = harness().await;
    h.messages(r#"{"model":"claude-test","messages":[]}"#, &[])
        .await;
    assert_eq!(
        h.call(Request::get("/metrics").body(Body::empty()).unwrap())
            .await
            .0,
        401
    );
    let req = Request::get("/metrics")
        .header("authorization", format!("Bearer {ADMIN}"))
        .body(Body::empty())
        .unwrap();
    let (status, headers, body) = h.call(req).await;
    assert_eq!(status, 200);
    assert!(hdr(&headers, "content-type").starts_with("text/plain"));
    let text = String::from_utf8(body).unwrap();
    assert!(
        text.contains(r#"ccm_requests_total{account_id="1",account="claude-cli",status="200"} 1"#),
        "{text}"
    );
    assert!(
        text.contains(r#"ccm_tokens_total{account_id="1",account="claude-cli",kind="input"} 7"#)
    );
    assert!(
        text.contains(r#"ccm_account_up{account_id="1",account="claude-cli",status="active"} 1"#)
    );
}

#[tokio::test]
async fn export_round_trips_through_import_and_tracks_last_used() {
    let h = harness().await;
    h.messages(r#"{"model":"claude-test","messages":[]}"#, &[])
        .await;
    assert!(h.core.accounts.get(1).unwrap().last_used_at > 0);

    let (status, out) = h
        .admin("GET", "/admin/api/accounts/export?ids=1", None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(out.as_array().unwrap().len(), 1);
    assert_eq!(
        out[0]["credentials"]["claudeAiOauth"]["accessToken"],
        "upstream-access"
    );
    assert_eq!(
        h.call(
            Request::get("/admin/api/accounts/export")
                .body(Body::empty())
                .unwrap()
        )
        .await
        .0,
        401
    );
    let (status, r) = h
        .admin(
            "POST",
            "/admin/api/accounts/import",
            Some(json!({"credentials": out})),
        )
        .await;
    assert_eq!(status, 200, "{r}");
    assert_eq!(r["imported"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn monitor_records_bodies_mapping_streams_and_rejections() {
    let h = harness().await;
    // Off by default: nothing is kept.
    h.messages(r#"{"model":"claude-test"}"#, &[]).await;
    let (_, m) = h.admin("GET", "/admin/api/monitor", None).await;
    assert_eq!(
        (m["enabled"].clone(), m["held"].clone()),
        (json!(false), json!(0))
    );

    let (s, _) = h
        .admin(
            "POST",
            "/admin/api/runtime",
            Some(json!({"monitor": {"enabled": true, "max_body_bytes": 4096}})),
        )
        .await;
    assert_eq!(s, 200);
    let (s, _) = h
        .admin(
            "POST",
            "/admin/api/settings/model-routes",
            Some(json!({"routes": [{"pattern": "claude-alias", "target": "claude-test"}]})),
        )
        .await;
    assert_eq!(s, 204);

    let (status, _, _) = h
        .messages(r#"{"model":"claude-alias","max_tokens":5,"messages":[{"role":"user","content":"hi"}]}"#, &[])
        .await;
    assert_eq!(status, 200);
    let (_, m) = h.admin("GET", "/admin/api/monitor", None).await;
    let item = &m["items"][0];
    assert_eq!(item["model"], "claude-alias");
    assert_eq!(item["mapped_model"], "claude-test");
    assert_eq!(item["status"], 200);
    assert_eq!(item["input_tokens"], 7);
    assert_eq!(item["user_label"], "alice");
    assert!(item["account_id"].as_i64().is_some());
    assert!(item["ttft_ms"].as_i64().is_some());
    assert!(item.get("request").is_none(), "list has no bodies");
    let id = item["id"].as_u64().unwrap();
    let (_, d) = h
        .admin("GET", &format!("/admin/api/monitor/{id}"), None)
        .await;
    assert!(d["request"].as_str().unwrap().contains("claude-alias"));
    let up = d["upstream_request"].as_str().unwrap();
    assert!(
        up.contains("\"claude-test\"") && up.contains("Claude Code"),
        "routed and injected: {up}"
    );
    assert!(d["response"].as_str().unwrap().contains("msg_1"));

    let sse = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":3}}}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":5}}\n\n",
    );
    h.set_reply(Reply::Sse(sse));
    h.messages(r#"{"model":"claude-test","stream":true}"#, &[])
        .await;
    let (_, m) = h
        .admin("GET", &format!("/admin/api/monitor?after_id={id}"), None)
        .await;
    assert_eq!(m["items"].as_array().unwrap().len(), 1);
    let sid = m["items"][0]["id"].as_u64().unwrap();
    assert_eq!(m["items"][0]["stream"], true);
    assert_eq!(m["items"][0]["output_tokens"], 5);
    let (_, d) = h
        .admin("GET", &format!("/admin/api/monitor/{sid}"), None)
        .await;
    assert!(d["response"]
        .as_str()
        .unwrap()
        .starts_with("event: message_start"));

    // A policy rejection never reaches upstream but is still monitored.
    h.admin(
        "POST",
        "/admin/api/settings/disabled-models",
        Some(json!({"models": ["claude-test"]})),
    )
    .await;
    let (status, _, _) = h.messages(r#"{"model":"claude-test"}"#, &[]).await;
    assert_eq!(status, 403);
    let (_, m) = h
        .admin("GET", "/admin/api/monitor?status=error", None)
        .await;
    assert_eq!(m["items"].as_array().unwrap().len(), 1);
    assert_eq!(m["items"][0]["status"], 403);
    assert!(m["items"][0]["account_id"].is_null());

    let (s, _) = h.admin("POST", "/admin/api/monitor/clear", None).await;
    assert_eq!(s, 204);
    let (_, m) = h.admin("GET", "/admin/api/monitor", None).await;
    assert_eq!(m["held"], 0);
}

#[tokio::test]
async fn runtime_settings_apply_live_and_validate() {
    let h = harness().await;
    let (_, r) = h.admin("GET", "/admin/api/runtime", None).await;
    assert_eq!(r["effective"]["pool"]["max_retries"], 2);
    assert_eq!(r["overrides"], json!({}));
    let (s, r) = h
        .admin(
            "POST",
            "/admin/api/runtime",
            Some(json!({"pool": {"max_retries": 0, "strategy": "round_robin"}})),
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(r["overrides"]["pool"]["max_retries"], 0);
    assert_eq!(h.core.pool.config().max_retries, 0);

    // With no retries a 429 is not failed over.
    let second = h.add_account("b", "b-tok", 0, "");
    h.reply_for(
        "upstream-access",
        Reply::Json(
            429,
            vec![],
            r#"{"type":"error","error":{"type":"rate_limit_error","message":"x"}}"#,
        ),
    );
    h.admin(
        "POST",
        "/admin/api/runtime",
        Some(json!({"pool": {"strategy": "priority"}})),
    )
    .await;
    h.core.accounts.pin(1).unwrap();
    let (status, _, _) = h.messages(r#"{"model":"claude-test"}"#, &[]).await;
    assert_eq!(status, 429);
    assert!(
        !h.tokens_seen().contains(&"b-tok".to_string()),
        "account #{second} untouched"
    );

    for bad in [
        json!({"pool": {"max_retry": 1}}),
        json!({"listen": "not an address"}),
        json!({"monitor": {"capacity": 0}}),
    ] {
        let (s, e) = h.admin("POST", "/admin/api/runtime", Some(bad)).await;
        assert_eq!(s, 400, "{e}");
    }
    // A null resets to the config value.
    h.admin(
        "POST",
        "/admin/api/runtime",
        Some(json!({"pool": {"max_retries": null}})),
    )
    .await;
    assert_eq!(h.core.pool.config().max_retries, 2);

    // The headless router has no listener controls.
    let (_, svc) = h.admin("GET", "/admin/api/service", None).await;
    assert_eq!(
        (svc["running"].clone(), svc["controllable"].clone()),
        (json!(false), json!(false))
    );
    let (s, _) = h.admin("POST", "/admin/api/service/stop", None).await;
    assert_eq!(s, 400);
}

#[tokio::test]
async fn model_test_goes_through_the_pool_without_stats() {
    let h = harness().await;
    h.set_reply(Reply::Json(
        200,
        vec![],
        r#"{"id":"m","content":[{"type":"text","text":"pong"}],"usage":{"input_tokens":1,"output_tokens":1}}"#,
    ));
    let (s, r) = h
        .admin(
            "POST",
            "/admin/api/proxy/test",
            Some(json!({"model": "claude-test"})),
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(
        (r["ok"].clone(), r["status"].clone(), r["text"].clone()),
        (json!(true), json!(200), json!("pong"))
    );
    assert_eq!(r["account_id"], 1);
    assert!(String::from_utf8_lossy(&h.last_seen("/v1/messages").body).contains("Claude Code"));

    h.set_reply(Reply::Json(
        400,
        vec![],
        r#"{"type":"error","error":{"type":"invalid_request_error","message":"no such model"}}"#,
    ));
    let (_, r) = h
        .admin(
            "POST",
            "/admin/api/proxy/test",
            Some(json!({"model": "nope"})),
        )
        .await;
    assert_eq!(r["ok"], false);
    assert!(r["error"].as_str().unwrap().contains("no such model"));
    h.core.recorder.flush();
    let (_, total) = h.core.recorder.search(&SearchFilter::default()).unwrap();
    assert_eq!(total, 0, "tests are not usage");
}

async fn bearer_status(h: &Harness, token: &str) -> StatusCode {
    let req = Request::post("/v1/messages")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from(r#"{"model":"claude-test"}"#))
        .unwrap();
    h.call(req).await.0
}

#[tokio::test]
async fn claude_code_sync_writes_a_working_key_and_restores() {
    let h = harness().await;
    let (_, st) = h.admin("GET", "/admin/api/claude-code", None).await;
    assert_eq!(
        (st["exists"].clone(), st["backup"].clone()),
        (json!(false), json!(false))
    );

    let token_in_file = || {
        let path = h.core.config.claude_settings_path.clone();
        let v: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        v["env"]["ANTHROPIC_AUTH_TOKEN"]
            .as_str()
            .unwrap()
            .to_string()
    };

    let (s, st) = h
        .admin(
            "POST",
            "/admin/api/claude-code/sync",
            Some(json!({"base_url": "http://127.0.0.1:8787"})),
        )
        .await;
    assert_eq!(s, 200, "{st}");
    assert_eq!(st["base_url"], "http://127.0.0.1:8787");
    assert!(st.get("token").is_none(), "the key is never returned");
    let first = token_in_file();
    assert!(first.starts_with("sk-cp-"));
    assert_eq!(bearer_status(&h, &first).await, 200);

    // Re-syncing rotates the key.
    h.admin(
        "POST",
        "/admin/api/claude-code/sync",
        Some(json!({"base_url": "http://127.0.0.1:8787"})),
    )
    .await;
    let second = token_in_file();
    assert_ne!(first, second);
    assert_eq!(bearer_status(&h, &first).await, 401);
    assert_eq!(bearer_status(&h, &second).await, 200);

    let (s, st) = h
        .admin("POST", "/admin/api/claude-code/restore", None)
        .await;
    assert_eq!(s, 200);
    assert_eq!(st["exists"], false);
    assert_eq!(
        bearer_status(&h, &second).await,
        401,
        "restore revokes the local key"
    );
    let (s, _) = h
        .admin("POST", "/admin/api/claude-code/restore", None)
        .await;
    assert_eq!(s, 400);
}

/// A messages call that appears to come from `ip` (as the real listener's
/// connect info would say), with extra headers.
async fn from_ip(h: &Harness, key: &str, ip: &str, extra: &[(&str, &str)]) -> (StatusCode, Value) {
    let mut b = Request::post("/v1/messages")
        .header("x-api-key", key)
        .header("content-type", "application/json");
    for (k, v) in extra {
        b = b.header(*k, *v);
    }
    let mut req = b.body(Body::from(r#"{"model":"claude-test"}"#)).unwrap();
    let addr: std::net::SocketAddr = format!("{ip}:50000").parse().unwrap();
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(addr));
    let (status, _, body) = h.call(req).await;
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

#[tokio::test]
async fn key_expiry_and_renewal() {
    let h = harness().await;
    let (s, k) = h
        .admin(
            "POST",
            "/admin/api/keys/issue",
            Some(json!({"user_id": h.user_id, "ttl_days": 3})),
        )
        .await;
    assert_eq!(s, 200);
    let key = k["api_key"].as_str().unwrap().to_string();
    let key_id = k["key_id"].as_i64().unwrap();
    assert_eq!(bearer_status(&h, &key).await, 200);
    let (_, users) = h.admin("GET", "/admin/api/users", None).await;
    let exp = users["users"][0]["keys"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["id"] == key_id)
        .unwrap()["expires_at"]
        .as_i64()
        .unwrap();
    assert!((exp - ccm_core_now() - 3 * 86_400).abs() < 60);

    // Expired keys are rejected even when the lookup is cached.
    h.admin(
        "POST",
        &format!("/admin/api/keys/{key_id}/expiry"),
        Some(json!({"expires_at": ccm_core_now() - 1})),
    )
    .await;
    let req = Request::post("/v1/messages")
        .header("x-api-key", &key)
        .body(Body::from(r#"{"model":"claude-test"}"#))
        .unwrap();
    let (status, _, body) = h.call(req).await;
    assert_eq!(status, 401);
    assert!(json(&body)["error"]["message"]
        .as_str()
        .unwrap()
        .contains("expired"));
    let (s, _) = h
        .admin(
            "POST",
            &format!("/admin/api/keys/{key_id}/expiry"),
            Some(json!({"days": 0})),
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(bearer_status(&h, &key).await, 200, "cleared expiry");
    let (s, _) = h
        .admin(
            "POST",
            "/admin/api/keys/999/expiry",
            Some(json!({"days": 1})),
        )
        .await;
    assert_eq!(s, 404);
}

#[tokio::test]
async fn ip_limit_tracks_distinct_ips_per_terminal() {
    let h = harness().await;
    let (s, _) = h
        .admin(
            "POST",
            &format!("/admin/api/users/{}/policy", h.user_id),
            Some(json!({"max_ips": 1})),
        )
        .await;
    assert_eq!(s, 204);
    let (_, p) = h
        .admin(
            "GET",
            &format!("/admin/api/users/{}/policy", h.user_id),
            None,
        )
        .await;
    assert_eq!(p["max_ips"], 1);

    assert_eq!(from_ip(&h, &h.api_key, "10.0.0.1", &[]).await.0, 200);
    assert_eq!(from_ip(&h, &h.api_key, "10.0.0.1", &[]).await.0, 200);
    let (s, e) = from_ip(&h, &h.api_key, "10.0.0.2", &[]).await;
    assert_eq!(s, 403);
    assert_eq!(e["error"]["type"], "permission_error");
    // X-Forwarded-For is ignored until trusted.
    assert_eq!(
        from_ip(
            &h,
            &h.api_key,
            "10.0.0.1",
            &[("x-forwarded-for", "10.0.0.9")]
        )
        .await
        .0,
        200
    );
    h.admin(
        "POST",
        "/admin/api/runtime",
        Some(json!({"security": {"trust_forwarded_for": true}})),
    )
    .await;
    let (s, _) = from_ip(
        &h,
        &h.api_key,
        "10.0.0.1",
        &[("x-forwarded-for", "10.0.0.9, 10.0.0.1")],
    )
    .await;
    assert_eq!(s, 403, "the forwarded client IP counts");

    let (_, ips) = h
        .admin("GET", &format!("/admin/api/users/{}/ips", h.user_id), None)
        .await;
    let ips = ips["ips"].as_array().unwrap();
    assert_eq!(ips.len(), 1);
    assert_eq!(
        (ips[0]["ip"].clone(), ips[0]["requests"].clone()),
        (json!("10.0.0.1"), json!(3))
    );
}

#[tokio::test]
async fn stats_overview_prices_and_today_counts() {
    let h = harness().await;
    for _ in 0..3 {
        h.messages(r#"{"model":"claude-test"}"#, &[]).await;
    }
    h.core.recorder.flush();
    let (s, o) = h
        .admin(
            "GET",
            "/admin/api/stats/overview?days=1&granularity=hour&group=model&tz_offset=480",
            None,
        )
        .await;
    assert_eq!(s, 200, "{o}");
    assert_eq!(o["totals"]["requests"], 3);
    assert_eq!(o["totals"]["input_tokens"], 21);
    assert_eq!(o["by_model"][0]["key"], "claude-test");
    assert_eq!(o["series"][0]["key"], "claude-test");
    assert_eq!(o["totals"]["cost_usd"], 0.0, "no price matches claude-test");
    assert!(!o["heatmap"].as_array().unwrap().is_empty());

    let (s, _) = h
        .admin(
            "POST",
            "/admin/api/settings/prices",
            Some(json!({"prices": [{"pattern": "claude-test", "input": 1000000.0, "output": 0.0, "cache_read": 0.0, "cache_write": 0.0}]})),
        )
        .await;
    assert_eq!(s, 200);
    let (_, o) = h
        .admin("GET", "/admin/api/stats/overview?days=1", None)
        .await;
    assert!((o["totals"]["cost_usd"].as_f64().unwrap() - 21.0).abs() < 1e-6);
    let (s, _) = h
        .admin("POST", "/admin/api/settings/prices", Some(json!({"prices": [{"pattern": "re:(", "input": 1.0, "output": 1.0, "cache_read": 0.0, "cache_write": 0.0}]})))
        .await;
    assert_eq!(s, 400);

    let (_, users) = h.admin("GET", "/admin/api/users?tz_offset=0", None).await;
    assert_eq!(users["users"][0]["today_requests"], 3);
    assert_eq!(users["users"][0]["today_tokens"], 30);
    let (_, mine) = h
        .admin(
            "GET",
            &format!("/admin/api/stats/overview?days=1&user_id={}", h.user_id + 1),
            None,
        )
        .await;
    assert_eq!(mine["totals"]["requests"], 0);
}

async fn register_from(h: &Harness, ip: &str) -> StatusCode {
    let mut req = Request::post("/register")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"token":"nope","label":"x"}"#))
        .unwrap();
    let addr: std::net::SocketAddr = format!("{ip}:1").parse().unwrap();
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(addr));
    h.call(req).await.0
}

#[tokio::test]
async fn ip_rules_block_and_log_without_touching_admin() {
    let h = harness().await;
    h.admin(
        "POST",
        "/admin/api/runtime",
        Some(json!({"security": {"block_message": "go away"}})),
    )
    .await;
    let (s, r) = h
        .admin(
            "POST",
            "/admin/api/security/rules",
            Some(json!({"kind": "deny", "cidr": "10.0.0.5", "note": "abuse"})),
        )
        .await;
    assert_eq!(s, 200, "{r}");
    let deny_id = r["id"].as_i64().unwrap();
    let (s, e) = from_ip(&h, &h.api_key, "10.0.0.5", &[]).await;
    assert_eq!(s, 403);
    assert_eq!(
        (e["error"]["type"].clone(), e["error"]["message"].clone()),
        (json!("permission_error"), json!("go away"))
    );
    assert_eq!(register_from(&h, "10.0.0.5").await, 403);
    assert_eq!(from_ip(&h, &h.api_key, "10.0.0.6", &[]).await.0, 200);
    // The admin API is never filtered.
    assert_eq!(h.admin("GET", "/admin/api/status", None).await.0, 200);

    // Allowlist mode.
    h.admin(
        "POST",
        "/admin/api/security/rules",
        Some(json!({"kind": "allow", "cidr": "192.168.0.0/16"})),
    )
    .await;
    let (_, rules) = h.admin("GET", "/admin/api/security/rules", None).await;
    assert_eq!(rules["allowlist_mode"], true);
    assert_eq!(from_ip(&h, &h.api_key, "10.0.0.6", &[]).await.0, 403);
    assert_eq!(from_ip(&h, &h.api_key, "192.168.7.7", &[]).await.0, 200);
    let (s, _) = h
        .admin(
            "POST",
            "/admin/api/security/rules",
            Some(json!({"kind": "deny", "cidr": "1.2.3.4/40"})),
        )
        .await;
    assert_eq!(s, 400);

    let (_, log) = h
        .admin("GET", "/admin/api/security/log?filter=blocked", None)
        .await;
    assert_eq!(log["total"], 3);
    let (_, rules) = h.admin("GET", "/admin/api/security/rules", None).await;
    let deny = rules["rules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == deny_id)
        .unwrap()
        .clone();
    assert_eq!(deny["hits"], 2);
    let (_, ips) = h.admin("GET", "/admin/api/security/ips?days=1", None).await;
    let lan = ips["ips"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["ip"] == "192.168.7.7")
        .unwrap()
        .clone();
    assert_eq!(lan["terminals"], json!(["alice"]));
    assert_eq!(lan["blocked"], 0);

    let (s, _) = h
        .admin(
            "POST",
            &format!("/admin/api/security/rules/{deny_id}/delete"),
            None,
        )
        .await;
    assert_eq!(s, 204);
    let (s, _) = h
        .admin("POST", "/admin/api/security/rules/999/delete", None)
        .await;
    assert_eq!(s, 404);
}

#[tokio::test]
async fn proxy_list_assign_and_test() {
    let h = harness().await;
    let (s, _) = h
        .admin(
            "POST",
            "/admin/api/proxies",
            Some(json!({"url": "ftp://x"})),
        )
        .await;
    assert_eq!(s, 400);
    let (s, p) = h
        .admin(
            "POST",
            "/admin/api/proxies",
            Some(json!({"name": "dead", "url": "http://127.0.0.1:9"})),
        )
        .await;
    assert_eq!(s, 200);
    let id = p["id"].as_i64().unwrap();
    let (_, t) = h
        .admin("POST", &format!("/admin/api/proxies/{id}/test"), None)
        .await;
    assert_eq!(t["ok"], false);
    let second = h.add_account("b", "b-tok", 0, "");
    let (s, p) = h
        .admin(
            "POST",
            &format!("/admin/api/proxies/{id}/assign"),
            Some(json!({"account_ids": [1, second]})),
        )
        .await;
    assert_eq!(s, 200, "{p}");
    assert_eq!(p["accounts"], json!([1, second]));
    assert_eq!(
        h.core.accounts.get(second).unwrap().proxy_url,
        "http://127.0.0.1:9"
    );
    // Unlisted accounts go back to direct.
    h.admin(
        "POST",
        &format!("/admin/api/proxies/{id}/assign"),
        Some(json!({"account_ids": [second]})),
    )
    .await;
    assert_eq!(h.core.accounts.get(1).unwrap().proxy_url, "");
    let (_, list) = h.admin("GET", "/admin/api/proxies", None).await;
    assert_eq!(list["proxies"][0]["accounts"], json!([second]));
    let (s, _) = h
        .admin("POST", &format!("/admin/api/proxies/{id}/delete"), None)
        .await;
    assert_eq!(s, 204);
}

#[tokio::test]
async fn logs_update_check_and_system() {
    let h = harness().await;
    ccm_core::logbuf::buffer().push(&tracing::Level::WARN, "test", "marker line".into());
    let (_, l) = h
        .admin("GET", "/admin/api/logs?level=WARN&limit=10", None)
        .await;
    assert!(l["lines"]
        .as_array()
        .unwrap()
        .iter()
        .any(|x| x["message"] == "marker line"));
    let (s, u) = h.admin("GET", "/admin/api/update/check", None).await;
    assert_eq!(s, 200, "{u}");
    assert_eq!(
        (u["latest"].clone(), u["newer"].clone()),
        (json!("v99.1.0"), json!(true))
    );
    let (_, sys) = h.admin("GET", "/admin/api/system", None).await;
    assert_eq!(sys["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(sys["desktop"], false);
}

#[cfg(unix)]
#[tokio::test]
async fn cloudflare_tunnel_install_start_routes_and_stop() {
    let h = harness().await;
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let (s, _) = h
        .admin(
            "POST",
            "/admin/api/runtime",
            Some(json!({"tunnel": {"local_port": port}})),
        )
        .await;
    assert_eq!(s, 200);
    let (s, v) = h.admin("POST", "/admin/api/tunnel/install", None).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        (
            v["status"]["installed"].clone(),
            v["status"]["managed"].clone()
        ),
        (json!(true), json!(true))
    );
    assert!(v["status"]["version"].as_str().unwrap().contains("fake"));

    let wait_url = || async {
        for _ in 0..50 {
            let (_, v) = h.admin("GET", "/admin/api/tunnel", None).await;
            if let Some(u) = v["status"]["url"].as_str() {
                return u.to_string();
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        panic!("no tunnel URL");
    };
    let (s, v) = h.admin("POST", "/admin/api/tunnel/start", None).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["status"]["running"], true);
    assert_eq!(wait_url().await, "https://fake-tunnel.trycloudflare.com");
    let (_, logs) = h.admin("GET", "/admin/api/tunnel/logs", None).await;
    assert!(logs["lines"][0]
        .as_str()
        .unwrap()
        .contains(&format!("tunnel --url http://127.0.0.1:{port}")));

    // The tunnel listener serves the terminal routes, not the admin API.
    let base = format!("http://127.0.0.1:{port}");
    let http = reqwest::Client::new();
    let msg = |ip: Option<&'static str>| {
        let mut r = http
            .post(format!("{base}/v1/messages"))
            .header("x-api-key", &h.api_key)
            .body(r#"{"model":"claude-test"}"#);
        if let Some(ip) = ip {
            r = r.header("cf-connecting-ip", ip);
        }
        r.send()
    };
    assert_eq!(msg(Some("203.0.113.5")).await.unwrap().status(), 200);
    assert_eq!(
        http.get(format!("{base}/healthz"))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let admin_via_tunnel = || {
        http.get(format!("{base}/admin/api/status"))
            .bearer_auth(ADMIN)
            .send()
    };
    assert_eq!(admin_via_tunnel().await.unwrap().status(), 404);

    // IP rules see the real client from CF-Connecting-IP.
    h.admin(
        "POST",
        "/admin/api/security/rules",
        Some(json!({"kind": "deny", "cidr": "203.0.113.5"})),
    )
    .await;
    assert_eq!(msg(Some("203.0.113.5")).await.unwrap().status(), 403);
    assert_eq!(
        msg(None).await.unwrap().status(),
        200,
        "no header: the loopback peer"
    );
    let (_, log) = h
        .admin("GET", "/admin/api/security/log?filter=blocked", None)
        .await;
    assert_eq!(log["entries"][0]["ip"], "203.0.113.5");

    let (_, v) = h.admin("POST", "/admin/api/tunnel/stop", None).await;
    assert_eq!(v["status"]["running"], false);
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(
        http.get(format!("{base}/healthz")).send().await.is_err(),
        "listener closed"
    );

    // Token mode: needs a token, passes it by environment only.
    h.admin(
        "POST",
        "/admin/api/runtime",
        Some(json!({"tunnel": {"mode": "token", "expose_admin": true}})),
    )
    .await;
    let (s, e) = h.admin("POST", "/admin/api/tunnel/start", None).await;
    assert_eq!(s, 400);
    assert!(e["error"].as_str().unwrap().contains("token"));
    let (_, v) = h
        .admin(
            "POST",
            "/admin/api/tunnel/token",
            Some(json!({"token": "eyJ0b2tlbiI6InNlY3JldCJ9"})),
        )
        .await;
    assert_eq!(v["status"]["has_token"], true);
    assert!(
        !v.to_string().contains("eyJ0b2tlbiI6InNlY3JldCJ9"),
        "token never returned"
    );
    let (s, _) = h.admin("POST", "/admin/api/tunnel/start", None).await;
    assert_eq!(s, 200);
    assert_eq!(wait_url().await, "https://cc.example.com");
    let (_, logs) = h.admin("GET", "/admin/api/tunnel/logs", None).await;
    let first = logs["lines"][0].as_str().unwrap().to_string();
    assert!(
        first.contains("tunnel run") && !first.contains("eyJ0b2tl"),
        "{first}"
    );
    // expose_admin serves the admin API too (still behind the admin token).
    assert_eq!(admin_via_tunnel().await.unwrap().status(), 200);
    h.core.tunnel.stop();
}
