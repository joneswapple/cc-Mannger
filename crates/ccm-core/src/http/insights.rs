//! Stats-page and terminal-detail endpoints: the analytics overview, the
//! price table, key expiry, and the IPs a terminal has used.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use super::admin::{internal, ApiResult};
use super::{json_error, AdminAuth};
use crate::analytics::{default_prices, Granularity, GroupBy, Price, Scope};
use crate::Core;

pub(super) fn routes() -> Router<Core> {
    Router::new()
        .route("/stats/overview", get(overview))
        .route("/settings/prices", get(get_prices).post(set_prices))
        .route("/keys/:id/expiry", post(set_expiry))
        .route("/users/:id/ips", get(user_ips))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct OverviewQuery {
    days: Option<i64>,
    granularity: Granularity,
    group: GroupBy,
    user_id: Option<i64>,
    /// Minutes east of UTC (the browser's `-getTimezoneOffset()`).
    tz_offset: i64,
}

async fn overview(
    _: AdminAuth,
    State(core): State<Core>,
    Query(q): Query<OverviewQuery>,
) -> ApiResult {
    let days = q.days.filter(|d| *d > 0).unwrap_or(7).min(366);
    let scope = Scope {
        since: crate::now_unix() - days * 86_400,
        user_id: q.user_id,
        tz_offset_secs: q.tz_offset.clamp(-14 * 60, 14 * 60) * 60,
    };
    let core2 = core.clone();
    let o = tokio::task::spawn_blocking(move || {
        core2.analytics.overview(&scope, q.granularity, q.group)
    })
    .await
    .map_err(|e| internal(e.into()))?
    .map_err(internal)?;
    let mut v = serde_json::to_value(o).map_err(|e| internal(e.into()))?;
    v["days"] = json!(days);
    Ok(Json(v).into_response())
}

async fn get_prices(_: AdminAuth, State(core): State<Core>) -> ApiResult {
    Ok(Json(json!({
        "prices": core.analytics.prices().map_err(internal)?,
        "defaults": default_prices(),
    }))
    .into_response())
}

#[derive(Deserialize)]
struct PricesRequest {
    /// Empty restores the defaults.
    prices: Vec<Price>,
}

async fn set_prices(
    _: AdminAuth,
    State(core): State<Core>,
    Json(req): Json<PricesRequest>,
) -> ApiResult {
    core.analytics
        .set_prices(&req.prices)
        .map_err(|e| json_error(StatusCode::BAD_REQUEST, format!("{e:#}")))?;
    get_prices(AdminAuth, State(core)).await
}

#[derive(Deserialize)]
struct ExpiryRequest {
    /// Days from now; 0 or absent with `expires_at` null means never.
    #[serde(default)]
    days: i64,
    /// Explicit unix seconds; wins over `days`.
    #[serde(default)]
    expires_at: Option<i64>,
}

/// Renews (or clears) a key's expiry.
async fn set_expiry(
    _: AdminAuth,
    State(core): State<Core>,
    Path(id): Path<i64>,
    Json(req): Json<ExpiryRequest>,
) -> ApiResult {
    let at = req
        .expires_at
        .or((req.days > 0).then(|| crate::now_unix() + req.days * 86_400));
    core.keys
        .set_expiry(id, at)
        .map_err(|e| json_error(StatusCode::NOT_FOUND, format!("{e:#}")))?;
    Ok(Json(json!({"key_id": id, "expires_at": at})).into_response())
}

async fn user_ips(_: AdminAuth, State(core): State<Core>, Path(id): Path<i64>) -> ApiResult {
    Ok(Json(json!({"ips": core.policy.user_ips(id).map_err(internal)?})).into_response())
}
