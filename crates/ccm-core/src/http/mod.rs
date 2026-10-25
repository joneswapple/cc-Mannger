//! The HTTP server: the Anthropic-compatible proxy under `/v1`, invite
//! registration, and the admin JSON API under `/admin/api`.

mod admin;
mod proxy;

use std::future::Future;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use axum::extract::{DefaultBodyLimit, FromRequestParts, Request};
use axum::http::{request::Parts, HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;
use serde_json::json;

use crate::auth::{Identity, KeyError, KEY_PREFIX};
use crate::upstream::CatalogStatus;
use crate::Core;

/// Largest request body accepted on any route.
const MAX_BODY: usize = 16 << 20;
const CATALOG_REFRESH: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub version: &'static str,
    pub listen: String,
    pub accounts: i64,
    pub upstream: String,
    pub catalog: CatalogStatus,
}

impl Core {
    pub fn status(&self) -> Result<Status> {
        Ok(Status {
            version: crate::VERSION,
            listen: self.config.listen.clone(),
            accounts: self.accounts.count()?,
            upstream: self.pool.source(),
            catalog: self.catalog.status(),
        })
    }
}

pub fn router(core: Core) -> Router {
    Router::new()
        .route("/v1/messages", post(proxy::messages))
        .route("/v1/messages/count_tokens", post(proxy::count_tokens))
        .route("/v1/chat/completions", post(proxy::chat_completions))
        .route("/v1/models", get(proxy::models))
        .route("/register", post(admin::register))
        .route("/oauth/callback", get(admin::oauth_callback))
        .route("/healthz", get(healthz))
        .route("/metrics", get(metrics))
        .nest("/admin/api", admin::routes())
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .layer(middleware::from_fn(log_requests))
        .with_state(core)
}

/// Binds `core.config.listen`, starts the background refreshers, and serves
/// until `shutdown` resolves.
pub async fn serve(core: Core, shutdown: impl Future<Output = ()> + Send + 'static) -> Result<()> {
    serve_with(core, Router::new(), shutdown).await
}

/// [`serve`] with extra routes merged in (e.g. the embedded web UI).
pub async fn serve_with(
    core: Core,
    extra: Router,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<()> {
    let listen = core.config.listen.clone();
    let listener = tokio::net::TcpListener::bind(&listen)
        .await
        .with_context(|| format!("listen on {listen}"))?;
    core.start_background();
    tracing::info!("cc-manager listening on {listen}");
    axum::serve(listener, router(core).merge(extra))
        .with_graceful_shutdown(shutdown)
        .await?;
    Ok(())
}

impl Core {
    /// Starts periodic jobs (model catalog). Must run inside a Tokio
    /// runtime; repeated calls start nothing new.
    pub fn start_background(&self) {
        self.background.call_once(|| {
            self.catalog.start(CATALOG_REFRESH);
            let url = self.config.upstream.usage_url.clone();
            if !url.is_empty() {
                let every = Duration::from_secs(self.config.upstream.usage_poll_secs.max(60));
                let upstream = self.upstream.clone();
                tokio::spawn(async move {
                    let mut tick = tokio::time::interval(every);
                    loop {
                        tick.tick().await;
                        upstream.probe_usage(&url).await;
                    }
                });
            }
        });
    }
}

/// Request extension marking an in-process call (the desktop UI bridge).
/// It can only be attached by code in this process, never by a network
/// client, and satisfies [`AdminAuth`].
#[derive(Clone, Copy)]
pub struct TrustedLocal;

/// Runs one admin API request in-process, as the desktop app's UI bridge:
/// same routes and behaviour as over HTTP, without a token. Returns the
/// status and the JSON body (`null` when empty).
pub async fn call_local(
    app: &Router,
    method: &str,
    path: &str,
    body: Option<serde_json::Value>,
) -> Result<(u16, serde_json::Value)> {
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    if !path.starts_with("/admin/api/") {
        anyhow::bail!("only /admin/api/* is reachable through the local bridge");
    }
    let mut req = axum::http::Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            body.map(|b| b.to_string()).unwrap_or_default(),
        ))?;
    req.extensions_mut().insert(TrustedLocal);
    let resp = app.clone().oneshot(req).await?;
    let status = resp.status().as_u16();
    let bytes = resp.into_body().collect().await?.to_bytes();
    let value = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| serde_json::Value::String(String::from_utf8_lossy(&bytes).into()))
    };
    Ok((status, value))
}

/// Prometheus metrics; needs the admin token (Bearer works for scrapers).
async fn metrics(_: AdminAuth, axum::extract::State(core): axum::extract::State<Core>) -> Response {
    match core.metrics() {
        Ok(text) => (
            [("content-type", "text/plain; version=0.0.4; charset=utf-8")],
            text,
        )
            .into_response(),
        Err(e) => json_error(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
    }
}

/// Liveness only; details live behind the admin API.
async fn healthz() -> Json<serde_json::Value> {
    Json(json!({"status": "ok", "version": crate::VERSION}))
}

async fn log_requests(req: Request, next: Next) -> Response {
    let (method, path) = (req.method().clone(), req.uri().path().to_string());
    let start = Instant::now();
    let resp = next.run(req).await;
    tracing::info!(
        "{method} {path} {} {}ms",
        resp.status().as_u16(),
        start.elapsed().as_millis()
    );
    resp
}

/// An error in Anthropic's shape. Everything `/v1/*` returns must use it so
/// Claude Code surfaces the message.
pub(crate) fn anthropic_error(
    status: StatusCode,
    kind: &str,
    message: impl Into<String>,
) -> Response {
    (
        status,
        Json(json!({"type": "error", "error": {"type": kind, "message": message.into()}})),
    )
        .into_response()
}

/// A plain `{"error": "..."}` for the admin API and registration.
pub(crate) fn json_error(status: StatusCode, message: impl std::fmt::Display) -> Response {
    (status, Json(json!({"error": message.to_string()}))).into_response()
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("authorization")?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name)?.to_str().ok().filter(|v| !v.is_empty())
}

/// The terminal behind an `sk-cp-…` key (`x-api-key` or `Authorization:
/// Bearer`).
pub(crate) struct ApiIdentity(pub Identity);

#[axum::async_trait]
impl FromRequestParts<Core> for ApiIdentity {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, core: &Core) -> Result<Self, Response> {
        let key = header(&parts.headers, "x-api-key")
            .or_else(|| bearer(&parts.headers))
            .map(str::to_string);
        let Some(key) = key else {
            return Err(anthropic_error(
                StatusCode::UNAUTHORIZED,
                "authentication_error",
                "missing api key — set the x-api-key header to an sk-cp-… key issued by this proxy",
            ));
        };
        let keys = core.keys.clone();
        let k = key.clone();
        let result = tokio::task::spawn_blocking(move || keys.lookup(&k))
            .await
            .unwrap_or(Err(KeyError::Invalid));
        match result {
            Ok(id) => Ok(ApiIdentity(id)),
            Err(KeyError::Revoked) => Err(anthropic_error(
                StatusCode::UNAUTHORIZED,
                "authentication_error",
                "api key revoked",
            )),
            Err(KeyError::UserDisabled) => Err(anthropic_error(
                StatusCode::FORBIDDEN,
                "authentication_error",
                "user disabled",
            )),
            Err(KeyError::Invalid) => Err(anthropic_error(
                StatusCode::UNAUTHORIZED,
                "authentication_error",
                api_key_hint(&key),
            )),
        }
    }
}

/// A message tailored to whatever was sent as the key.
fn api_key_hint(key: &str) -> &'static str {
    if key.starts_with(KEY_PREFIX) {
        "invalid api key (no matching key found — was it revoked, or does it belong to a different CC Manager instance?)"
    } else if key.starts_with("sk-ant-") {
        "this looks like a real Anthropic api key; CC Manager expects its own sk-cp-… keys (issue one via POST /register with an invite token)"
    } else if key.len() >= 16 && key.chars().all(|c| c.is_ascii_hexdigit()) {
        "this looks like an invite token, not an api key — first redeem it: POST /register with body {\"token\":\"<this value>\",\"label\":\"...\"}, then use the returned api_key"
    } else {
        "invalid api key — CC Manager keys start with sk-cp-…; redeem an invite via POST /register to get one"
    }
}

/// Admin token from the `ccm_admin` cookie, `Authorization: Bearer`, or
/// `x-admin-token`.
pub(crate) struct AdminAuth;

#[axum::async_trait]
impl FromRequestParts<Core> for AdminAuth {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, core: &Core) -> Result<Self, Response> {
        if parts.extensions.get::<TrustedLocal>().is_some() {
            return Ok(AdminAuth);
        }
        let token = cookie(&parts.headers, "ccm_admin")
            .or_else(|| bearer(&parts.headers))
            .or_else(|| header(&parts.headers, "x-admin-token"))
            .unwrap_or_default()
            .to_string();
        let admin = core.admin.clone();
        let ok = tokio::task::spawn_blocking(move || admin.verify(&token).is_ok())
            .await
            .unwrap_or(false);
        if ok {
            Ok(AdminAuth)
        } else {
            Err(json_error(StatusCode::UNAUTHORIZED, "unauthorized"))
        }
    }
}

fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all("cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|kv| kv.trim().strip_prefix(name)?.strip_prefix('='))
        .filter(|v| !v.is_empty())
}
