//! `/admin/api/tunnel*`: public access through a Cloudflare tunnel.
//! Settings live in `runtime.tunnel` (`POST /admin/api/runtime`); the token
//! is set here and never returned.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use super::admin::ApiResult;
use super::{json_error, AdminAuth};
use crate::Core;

pub(super) fn routes() -> Router<Core> {
    Router::new()
        .route("/tunnel", get(status))
        .route("/tunnel/install", post(install))
        .route("/tunnel/start", post(start))
        .route("/tunnel/stop", post(stop))
        .route("/tunnel/token", post(set_token))
        .route("/tunnel/logs", get(logs))
}

async fn view(core: &Core) -> serde_json::Value {
    let cfg = core.runtime.get().tunnel;
    let status = core.tunnel.status(&cfg).await;
    json!({ "status": status, "config": cfg })
}

async fn status(_: AdminAuth, State(core): State<Core>) -> ApiResult {
    Ok(Json(view(&core).await).into_response())
}

async fn install(_: AdminAuth, State(core): State<Core>) -> ApiResult {
    let cfg = core.runtime.get().tunnel;
    core.tunnel
        .install(&cfg)
        .await
        .map_err(|e| json_error(StatusCode::BAD_GATEWAY, format!("{e:#}")))?;
    Ok(Json(view(&core).await).into_response())
}

async fn start(_: AdminAuth, State(core): State<Core>) -> ApiResult {
    core.start_tunnel()
        .await
        .map_err(|e| json_error(StatusCode::BAD_REQUEST, format!("{e:#}")))?;
    Ok(Json(view(&core).await).into_response())
}

async fn stop(_: AdminAuth, State(core): State<Core>) -> ApiResult {
    core.tunnel.stop();
    Ok(Json(view(&core).await).into_response())
}

#[derive(Deserialize)]
struct TokenRequest {
    /// The tunnel token from the Cloudflare dashboard; null / empty clears it.
    token: Option<String>,
}

async fn set_token(
    _: AdminAuth,
    State(core): State<Core>,
    Json(r): Json<TokenRequest>,
) -> ApiResult {
    core.tunnel
        .set_token(r.token.as_deref())
        .map_err(|e| json_error(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")))?;
    tracing::info!(
        "tunnel token {}",
        if core.tunnel.has_token() {
            "saved"
        } else {
            "cleared"
        }
    );
    Ok(Json(view(&core).await).into_response())
}

async fn logs(_: AdminAuth, State(core): State<Core>) -> ApiResult {
    Ok(Json(json!({ "lines": core.tunnel.logs() })).into_response())
}
