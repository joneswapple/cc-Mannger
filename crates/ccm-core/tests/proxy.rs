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
