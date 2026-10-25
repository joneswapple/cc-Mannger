//! `/admin/api/*` JSON endpoints and public invite registration. The
//! `ccm-server admin …` CLI is a thin wrapper over these. Timestamps are
//! unix seconds.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{json_error, AdminAuth};
use crate::accounts::{AccountPatch, NotFound};
use crate::auth::InviteError;
use crate::login::LoginIntent;
use crate::policy::Policy;
use crate::routing::ModelRoute;
use crate::stats::SearchFilter;
use crate::Core;

pub(super) fn routes() -> Router<Core> {
    Router::new()
        .merge(super::ops::routes())
        .route("/status", get(status))
        .route("/users", get(list_users).post(create_user))
        .route("/users/:id/enable", post(enable_user))
        .route("/users/:id/disable", post(disable_user))
        .route("/users/:id/policy", get(get_policy).post(set_policy))
        .route("/keys/issue", post(issue_key))
        .route("/keys/:id/revoke", post(revoke_key))
        .route("/invites", get(list_invites).post(create_invite))
        .route("/invites/:hash/delete", post(delete_invite))
        .route("/accounts", get(list_accounts))
        .route("/accounts/oauth/start", post(oauth_start))
        .route("/accounts/oauth/complete", post(oauth_complete))
        .route("/accounts/import", post(import_accounts))
        .route("/accounts/export", get(export_accounts))
        .route("/accounts/:id", post(update_account))
        .route("/accounts/:id/refresh", post(refresh_account))
        .route("/accounts/:id/pin", post(pin_account))
        .route("/accounts/:id/delete", post(delete_account))
        .route("/settings", get(settings))
        .route("/settings/disabled-models", post(set_disabled_models))
        .route("/settings/refresh-models", post(refresh_models))
        .route("/settings/model-routes", post(set_model_routes))
        .route("/stats", get(stats))
        .route("/usage", get(usage))
}

pub(super) type ApiResult = Result<Response, Response>;

pub(super) fn internal(e: anyhow::Error) -> Response {
    json_error(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}"))
}

fn no_content() -> Response {
    StatusCode::NO_CONTENT.into_response()
}

/// Runs CPU-heavy (bcrypt) work off the async workers.
pub(super) async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
) -> anyhow::Result<T> {
    tokio::task::spawn_blocking(f).await?
}

#[derive(Deserialize)]
pub(super) struct RegisterRequest {
    #[serde(default)]
    token: String,
    #[serde(default)]
    label: String,
}

/// Redeems an invite for a user and its first API key (shown once).
pub(super) async fn register(
    State(core): State<Core>,
    body: Result<Json<RegisterRequest>, axum::extract::rejection::JsonRejection>,
) -> ApiResult {
    let Json(req) =
        body.map_err(|e| json_error(StatusCode::BAD_REQUEST, format!("invalid json: {e}")))?;
    let invites = core.invites.clone();
    match blocking(move || invites.redeem(&req.token, &req.label)).await {
        Ok(r) => Ok(Json(json!({"api_key": r.api_key, "user_id": r.user_id})).into_response()),
        Err(e) if e.is::<InviteError>() => Err(json_error(StatusCode::BAD_REQUEST, e)),
        Err(e) => Err(internal(e)),
    }
}

async fn status(_: AdminAuth, State(core): State<Core>) -> ApiResult {
    Ok(Json(core.status().map_err(internal)?).into_response())
}

#[derive(Serialize)]
struct UserView {
    #[serde(flatten)]
    user: crate::auth::UserKeys,
    policy: Policy,
}

async fn list_users(_: AdminAuth, State(core): State<Core>) -> ApiResult {
    let users = core.keys.list_users().map_err(internal)?;
    let out = users
        .into_iter()
        .map(|u| {
            let policy = core.policy.get_policy(u.id)?;
            Ok(UserView { user: u, policy })
        })
        .collect::<anyhow::Result<Vec<_>>>()
        .map_err(internal)?;
    Ok(Json(json!({"users": out})).into_response())
}

#[derive(Deserialize)]
struct CreateUserRequest {
    label: String,
}

async fn create_user(
    _: AdminAuth,
    State(core): State<Core>,
    Json(req): Json<CreateUserRequest>,
) -> ApiResult {
    let id = core
        .keys
        .create_user(&req.label)
        .map_err(|e| json_error(StatusCode::BAD_REQUEST, e))?;
    Ok(Json(json!({"user_id": id})).into_response())
}

async fn set_user_enabled(core: &Core, id: i64, enabled: bool) -> ApiResult {
    if !core.keys.user_exists(id).map_err(internal)? {
        return Err(json_error(StatusCode::NOT_FOUND, "user not found"));
    }
    core.keys.set_user_enabled(id, enabled).map_err(internal)?;
    Ok(no_content())
}

async fn enable_user(_: AdminAuth, State(core): State<Core>, Path(id): Path<i64>) -> ApiResult {
    set_user_enabled(&core, id, true).await
}

async fn disable_user(_: AdminAuth, State(core): State<Core>, Path(id): Path<i64>) -> ApiResult {
    set_user_enabled(&core, id, false).await
}

async fn get_policy(_: AdminAuth, State(core): State<Core>, Path(id): Path<i64>) -> ApiResult {
    Ok(Json(core.policy.get_policy(id).map_err(internal)?).into_response())
}

async fn set_policy(
    _: AdminAuth,
    State(core): State<Core>,
    Path(id): Path<i64>,
    Json(p): Json<Policy>,
) -> ApiResult {
    if !core.keys.user_exists(id).map_err(internal)? {
        return Err(json_error(StatusCode::NOT_FOUND, "user not found"));
    }
    core.policy.set_policy(id, &p).map_err(internal)?;
    Ok(no_content())
}

#[derive(Deserialize)]
struct IssueKeyRequest {
    #[serde(default)]
    user_id: i64,
}

async fn issue_key(
    _: AdminAuth,
    State(core): State<Core>,
    Json(req): Json<IssueKeyRequest>,
) -> ApiResult {
    if req.user_id <= 0 {
        return Err(json_error(StatusCode::BAD_REQUEST, "user_id required"));
    }
    if !core.keys.user_exists(req.user_id).map_err(internal)? {
        return Err(json_error(StatusCode::NOT_FOUND, "user not found"));
    }
    let keys = core.keys.clone();
    let (key, id) = blocking(move || keys.issue(req.user_id))
        .await
        .map_err(internal)?;
    Ok(Json(json!({"api_key": key, "key_id": id})).into_response())
}

async fn revoke_key(_: AdminAuth, State(core): State<Core>, Path(id): Path<i64>) -> ApiResult {
    core.keys.revoke(id).map_err(internal)?;
    Ok(no_content())
}

async fn list_invites(_: AdminAuth, State(core): State<Core>) -> ApiResult {
    Ok(Json(json!({"invites": core.invites.list().map_err(internal)?})).into_response())
}

#[derive(Deserialize)]
struct CreateInviteRequest {
    #[serde(default)]
    label: String,
    #[serde(default)]
    ttl_hours: i64,
}

async fn create_invite(
    _: AdminAuth,
    State(core): State<Core>,
    Json(req): Json<CreateInviteRequest>,
) -> ApiResult {
    let ttl_hours = if req.ttl_hours > 0 { req.ttl_hours } else { 24 };
    let (token, expires_at) = core
        .invites
        .create(&req.label, ttl_hours * 3600)
        .map_err(|e| json_error(StatusCode::BAD_REQUEST, e))?;
    Ok(Json(json!({"token": token, "expires_at": expires_at})).into_response())
}

async fn delete_invite(
    _: AdminAuth,
    State(core): State<Core>,
    Path(hash): Path<String>,
) -> ApiResult {
    core.invites.delete_by_hash(&hash).map_err(internal)?;
    Ok(no_content())
}

/// Account metadata only; tokens never leave the store. `now` lets clients
/// turn `cooldown_until` into a countdown.
async fn list_accounts(_: AdminAuth, State(core): State<Core>) -> ApiResult {
    let quotas = core.pool.quotas().map_err(internal)?;
    let accounts: Vec<serde_json::Value> = core
        .accounts
        .list()
        .map_err(internal)?
        .into_iter()
        .map(|a| {
            let quota = quotas.get(&a.id);
            let mut v = serde_json::to_value(&a).unwrap_or_default();
            v["quota"] = serde_json::to_value(quota).unwrap_or_default();
            v
        })
        .collect();
    Ok(Json(json!({
        "accounts": accounts,
        "now": crate::now_unix(),
        "pool": core.pool.config(),
        "recommended": core.pool.preview().map_err(internal)?.map(|a| a.id),
    }))
    .into_response())
}

fn account_error(e: anyhow::Error) -> Response {
    if e.is::<NotFound>() {
        json_error(StatusCode::NOT_FOUND, e)
    } else {
        json_error(StatusCode::BAD_REQUEST, format!("{e:#}"))
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct IntentRequest {
    label: String,
    account_group: String,
}

impl From<IntentRequest> for LoginIntent {
    fn from(r: IntentRequest) -> Self {
        LoginIntent {
            label: r.label,
            account_group: r.account_group,
        }
    }
}

/// Starts an OAuth login: open `url`, then post the shown `code#state` to
/// `/accounts/oauth/complete` (or let the redirect hit `/oauth/callback`).
async fn oauth_start(
    _: AdminAuth,
    State(core): State<Core>,
    body: Option<Json<IntentRequest>>,
) -> ApiResult {
    let intent = body.map(|Json(b)| b).unwrap_or_default();
    Ok(Json(core.login.start(intent.into())).into_response())
}

#[derive(Deserialize)]
struct CompleteRequest {
    code: String,
    #[serde(default)]
    state: Option<String>,
}

async fn oauth_complete(
    _: AdminAuth,
    State(core): State<Core>,
    Json(req): Json<CompleteRequest>,
) -> ApiResult {
    core.complete_login(&req.code, req.state.as_deref())
        .await
        .map(|a| Json(a).into_response())
        .map_err(|e| json_error(StatusCode::BAD_REQUEST, format!("{e:#}")))
}

#[derive(Deserialize)]
struct ImportRequest {
    /// One credentials document, or an array (see `Core::import_batch`).
    credentials: serde_json::Value,
    #[serde(default)]
    label: String,
    #[serde(default)]
    account_group: String,
}

async fn import_accounts(
    _: AdminAuth,
    State(core): State<Core>,
    Json(req): Json<ImportRequest>,
) -> ApiResult {
    let results = core
        .import_batch(
            &req.credentials,
            LoginIntent {
                label: req.label,
                account_group: req.account_group,
            },
        )
        .await;
    let (mut imported, mut errors) = (vec![], vec![]);
    for (i, r) in results.into_iter().enumerate() {
        match r {
            Ok(a) => imported.push(a),
            Err(e) => errors.push(json!({"index": i, "error": e})),
        }
    }
    let status = if imported.is_empty() && !errors.is_empty() {
        StatusCode::BAD_REQUEST
    } else {
        StatusCode::OK
    };
    Ok((
        status,
        Json(json!({"imported": imported, "errors": errors})),
    )
        .into_response())
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ExportQuery {
    /// Comma-separated account ids; empty exports every account.
    ids: String,
}

/// The one endpoint that returns token material: a batch in the format
/// `/accounts/import` accepts, for backup or moving accounts elsewhere.
async fn export_accounts(
    _: AdminAuth,
    State(core): State<Core>,
    Query(q): Query<ExportQuery>,
) -> ApiResult {
    let wanted: Vec<i64> = q
        .ids
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();
    let mut out = vec![];
    for a in core.accounts.list().map_err(internal)? {
        if !wanted.is_empty() && !wanted.contains(&a.id) {
            continue;
        }
        let creds = core.accounts.load_credentials(a.id).map_err(internal)?;
        out.push(json!({
            "label": a.label,
            "account_group": a.account_group,
            "credentials": {"claudeAiOauth": creds},
        }));
    }
    tracing::info!("exported {} account(s) with credentials", out.len());
    Ok(Json(out).into_response())
}

async fn refresh_account(_: AdminAuth, State(core): State<Core>, Path(id): Path<i64>) -> ApiResult {
    core.refresh_account(id)
        .await
        .map(|a| Json(a).into_response())
        .map_err(account_error)
}

async fn pin_account(_: AdminAuth, State(core): State<Core>, Path(id): Path<i64>) -> ApiResult {
    let a = core.accounts.pin(id).map_err(account_error)?;
    // A pin is a manual switch: move bound sessions over too.
    core.pool.clear_sticky();
    Ok(Json(a).into_response())
}

async fn delete_account(_: AdminAuth, State(core): State<Core>, Path(id): Path<i64>) -> ApiResult {
    core.accounts.delete(id).map_err(account_error)?;
    core.pool.forget(id);
    Ok(no_content())
}

/// Public: the OAuth redirect target when `oauth_redirect_uri` points here.
/// The unguessable, single-use `state` is what authorizes it.
pub(super) async fn oauth_callback(
    State(core): State<Core>,
    Query(q): Query<CallbackQuery>,
) -> Response {
    let (status, msg) = match core.complete_login(&q.code, Some(&q.state)).await {
        Ok(a) => (
            StatusCode::OK,
            format!("已添加账号：{}（{}）。可以关闭此页面。", a.label, a.email),
        ),
        Err(e) => (StatusCode::BAD_REQUEST, format!("登录失败：{e:#}")),
    };
    let html = format!(
        "<!doctype html><meta charset=utf-8><title>CC Manager</title><p style=\"font:16px system-ui;margin:3em\">{}</p>",
        html_escape(&msg)
    );
    (status, axum::response::Html(html)).into_response()
}

#[derive(Deserialize)]
pub(super) struct CallbackQuery {
    #[serde(default)]
    code: String,
    #[serde(default)]
    state: String,
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

async fn update_account(
    _: AdminAuth,
    State(core): State<Core>,
    Path(id): Path<i64>,
    Json(patch): Json<AccountPatch>,
) -> ApiResult {
    match core.accounts.update(id, &patch) {
        Ok(a) => Ok(Json(a).into_response()),
        Err(e) if e.is::<NotFound>() => Err(json_error(StatusCode::NOT_FOUND, e)),
        Err(e) => Err(json_error(StatusCode::BAD_REQUEST, format!("{e:#}"))),
    }
}

async fn settings(_: AdminAuth, State(core): State<Core>) -> ApiResult {
    let up = &core.config.upstream;
    Ok(Json(json!({
        "disabled_models": core.policy.disabled_models().map_err(internal)?,
        "model_routes": core.routing.routes().map_err(internal)?,
        "routing": core.config.routing,
        "models": core.catalog.models(),
        "catalog": core.catalog.status(),
        "upstream": {
            "source": core.upstream.source(),
            "base_url": up.base_url,
            "anthropic_beta": up.anthropic_beta,
            "oauth_token_url": up.oauth_token_url,
            "oauth_client_id": up.oauth_client_id,
            "user_agent": up.user_agent,
            "inject_claude_code_system": up.inject_claude_code_system,
            "effort_levels": up.effort_levels,
        },
    }))
    .into_response())
}

#[derive(Deserialize)]
struct DisabledModelsRequest {
    #[serde(default)]
    models: Vec<String>,
}

async fn set_disabled_models(
    _: AdminAuth,
    State(core): State<Core>,
    Json(req): Json<DisabledModelsRequest>,
) -> ApiResult {
    core.policy
        .set_disabled_models(&req.models)
        .map_err(internal)?;
    Ok(no_content())
}

#[derive(Deserialize)]
struct ModelRoutesRequest {
    #[serde(default)]
    routes: Vec<ModelRoute>,
}

async fn set_model_routes(
    _: AdminAuth,
    State(core): State<Core>,
    Json(req): Json<ModelRoutesRequest>,
) -> ApiResult {
    core.routing
        .set_routes(&req.routes)
        .map_err(|e| json_error(StatusCode::BAD_REQUEST, format!("{e:#}")))?;
    Ok(no_content())
}

async fn refresh_models(_: AdminAuth, State(core): State<Core>) -> ApiResult {
    core.catalog
        .refresh()
        .await
        .map_err(|e| json_error(StatusCode::BAD_GATEWAY, format!("{e:#}")))?;
    Ok(Json(json!({"models": core.catalog.ids()})).into_response())
}

#[derive(Deserialize)]
struct StatsQuery {
    days: Option<i64>,
}

async fn stats(_: AdminAuth, State(core): State<Core>, Query(q): Query<StatsQuery>) -> ApiResult {
    let days = q.days.filter(|d| *d > 0).unwrap_or(30);
    let r = &core.recorder;
    Ok(Json(json!({
        "days": days,
        "by_day": r.by_day(days).map_err(internal)?,
        "by_user": r.by_user(days).map_err(internal)?,
        "by_model": r.by_model(days).map_err(internal)?,
        "by_account": r.by_account(days).map_err(internal)?,
    }))
    .into_response())
}

/// Query string accepted by `/admin/api/usage`; same names as the Go
/// cliproxy dashboard filters.
#[derive(Deserialize, Default)]
#[serde(default)]
struct UsageQuery {
    q: String,
    user_id: Option<i64>,
    account_id: Option<i64>,
    model: String,
    status: String,
    only_error: String,
    min_tokens: Option<i64>,
    sort: String,
    since: String,
    until: String,
    page: Option<i64>,
    page_size: Option<i64>,
}

/// Unix seconds, or `YYYY-MM-DDTHH:MM` read as UTC.
fn parse_time(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    s.parse::<i64>().ok().or_else(|| {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M")
            .ok()
            .map(|t| t.and_utc().timestamp())
    })
}

async fn usage(_: AdminAuth, State(core): State<Core>, Query(q): Query<UsageQuery>) -> ApiResult {
    let page = q.page.filter(|p| *p > 0).unwrap_or(1);
    let page_size = q.page_size.filter(|p| *p > 0).unwrap_or(50).min(500);
    let filter = SearchFilter {
        text: q.q,
        user_id: q.user_id.unwrap_or(0),
        account_id: q.account_id.unwrap_or(0),
        model: q.model,
        status: q.status,
        since: parse_time(&q.since),
        until: parse_time(&q.until),
        min_tokens: q.min_tokens.unwrap_or(0),
        only_error: matches!(q.only_error.as_str(), "1" | "true" | "on"),
        sort: q.sort,
        limit: page_size,
        offset: (page - 1) * page_size,
    };
    let (events, total) = core.recorder.search(&filter).map_err(internal)?;
    Ok(
        Json(json!({"events": events, "total": total, "page": page, "page_size": page_size}))
            .into_response(),
    )
}

#[cfg(test)]
mod tests {
    use super::parse_time;

    #[test]
    fn parse_time_forms() {
        assert_eq!(parse_time("1700000000"), Some(1_700_000_000));
        assert_eq!(parse_time("2024-01-02T03:04"), Some(1_704_164_640));
        assert_eq!(parse_time(""), None);
        assert_eq!(parse_time("garbage"), None);
    }
}
