//! Settings-center and security endpoints: IP rules, the access log and IP
//! statistics, the shared proxy list, the log viewer, the update check and
//! system information.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use super::admin::{blocking, internal, ApiResult};
use super::{json_error, AdminAuth};
use crate::accounts::AccountPatch;
use crate::proxies::ProxyList;
use crate::security::{LogFilter, RuleKind};
use crate::Core;

pub(super) fn routes() -> Router<Core> {
    Router::new()
        .route("/security/rules", get(list_rules).post(add_rule))
        .route("/security/rules/:id/delete", post(delete_rule))
        .route("/security/log", get(access_log))
        .route("/security/ips", get(ip_stats))
        .route("/proxies", get(list_proxies).post(add_proxy))
        .route("/proxies/test", post(test_proxy_url))
        .route("/proxies/:id/delete", post(delete_proxy))
        .route("/proxies/:id/test", post(test_proxy))
        .route("/proxies/:id/assign", post(assign_proxy))
        .route("/logs", get(logs))
        .route("/logs/clear", post(clear_logs))
        .route("/update/check", get(update_check))
        .route("/system", get(system))
}

fn bad_request(e: anyhow::Error) -> Response {
    json_error(StatusCode::BAD_REQUEST, format!("{e:#}"))
}

async fn list_rules(_: AdminAuth, State(core): State<Core>) -> ApiResult {
    let s = core.security.clone();
    let rules = blocking(move || s.rules()).await.map_err(internal)?;
    let now = crate::now_unix();
    let allowlist = rules
        .iter()
        .any(|r| r.kind == RuleKind::Allow && r.expires_at.is_none_or(|t| t > now));
    Ok(Json(json!({
        "rules": rules,
        "allowlist_mode": allowlist,
        "block_message": core.runtime.get().security.block_message,
    }))
    .into_response())
}

#[derive(Deserialize)]
struct RuleRequest {
    kind: RuleKind,
    cidr: String,
    #[serde(default)]
    note: String,
    /// Hours until the rule lapses; absent or 0 = permanent.
    #[serde(default)]
    expires_hours: i64,
}

async fn add_rule(_: AdminAuth, State(core): State<Core>, Json(r): Json<RuleRequest>) -> ApiResult {
    let expires = (r.expires_hours > 0).then(|| crate::now_unix() + r.expires_hours * 3600);
    let id = core
        .security
        .add_rule(r.kind, &r.cidr, &r.note, expires)
        .map_err(bad_request)?;
    tracing::info!("ip rule #{id} added: {:?} {}", r.kind, r.cidr.trim());
    Ok(Json(json!({"id": id})).into_response())
}

async fn delete_rule(_: AdminAuth, State(core): State<Core>, Path(id): Path<i64>) -> ApiResult {
    core.security
        .delete_rule(id)
        .map_err(|e| json_error(StatusCode::NOT_FOUND, format!("{e:#}")))?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct LogQuery {
    ip: String,
    /// `blocked` | `allowed` | empty for both.
    filter: String,
    page: i64,
    page_size: i64,
}

async fn access_log(
    _: AdminAuth,
    State(core): State<Core>,
    Query(q): Query<LogQuery>,
) -> ApiResult {
    let page = q.page.max(1);
    let size = if q.page_size <= 0 {
        50
    } else {
        q.page_size.min(500)
    };
    let f = LogFilter {
        ip: q.ip,
        blocked: match q.filter.as_str() {
            "blocked" => Some(true),
            "allowed" => Some(false),
            _ => None,
        },
        limit: size,
        offset: (page - 1) * size,
    };
    let s = core.security.clone();
    let (rows, total) = blocking(move || s.access_log(&f)).await.map_err(internal)?;
    Ok(
        Json(json!({"entries": rows, "total": total, "page": page, "page_size": size}))
            .into_response(),
    )
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct IpQuery {
    days: Option<i64>,
}

async fn ip_stats(_: AdminAuth, State(core): State<Core>, Query(q): Query<IpQuery>) -> ApiResult {
    let days = q.days.filter(|d| *d > 0).unwrap_or(7).min(365);
    let s = core.security.clone();
    let since = crate::now_unix() - days * 86_400;
    let stats = blocking(move || s.ip_stats(since, 500))
        .await
        .map_err(internal)?;
    Ok(Json(json!({"days": days, "ips": stats})).into_response())
}

async fn list_proxies(_: AdminAuth, State(core): State<Core>) -> ApiResult {
    Ok(Json(json!({"proxies": core.proxies.list().map_err(internal)?})).into_response())
}

#[derive(Deserialize)]
struct ProxyRequest {
    #[serde(default)]
    name: String,
    url: String,
}

async fn add_proxy(
    _: AdminAuth,
    State(core): State<Core>,
    Json(r): Json<ProxyRequest>,
) -> ApiResult {
    let id = core.proxies.add(&r.name, &r.url).map_err(bad_request)?;
    Ok(Json(json!({"id": id})).into_response())
}

async fn delete_proxy(_: AdminAuth, State(core): State<Core>, Path(id): Path<i64>) -> ApiResult {
    core.proxies
        .delete(id)
        .map_err(|e| json_error(StatusCode::NOT_FOUND, format!("{e:#}")))?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn test_proxy(_: AdminAuth, State(core): State<Core>, Path(id): Path<i64>) -> ApiResult {
    let p = core
        .proxies
        .get(id)
        .map_err(|e| json_error(StatusCode::NOT_FOUND, format!("{e:#}")))?;
    let r = ProxyList::test(&p.url, &core.config.upstream.base_url).await;
    Ok(Json(r).into_response())
}

#[derive(Deserialize)]
struct TestUrlRequest {
    url: String,
}

/// Tests a proxy URL before it is saved.
async fn test_proxy_url(
    _: AdminAuth,
    State(core): State<Core>,
    Json(r): Json<TestUrlRequest>,
) -> ApiResult {
    crate::proxies::validate_url(r.url.trim()).map_err(bad_request)?;
    let res = ProxyList::test(r.url.trim(), &core.config.upstream.base_url).await;
    Ok(Json(res).into_response())
}

#[derive(Deserialize)]
struct AssignRequest {
    account_ids: Vec<i64>,
}

/// Points the given accounts at this proxy; accounts that used it but are
/// not listed go back to a direct connection.
async fn assign_proxy(
    _: AdminAuth,
    State(core): State<Core>,
    Path(id): Path<i64>,
    Json(r): Json<AssignRequest>,
) -> ApiResult {
    let p = core
        .proxies
        .get(id)
        .map_err(|e| json_error(StatusCode::NOT_FOUND, format!("{e:#}")))?;
    let set = |account: i64, url: &str| -> anyhow::Result<()> {
        core.accounts.update(
            account,
            &AccountPatch {
                proxy_url: Some(url.to_string()),
                ..Default::default()
            },
        )?;
        core.pool.forget(account);
        Ok(())
    };
    for account in &p.accounts {
        if !r.account_ids.contains(account) {
            set(*account, "").map_err(bad_request)?;
        }
    }
    for account in &r.account_ids {
        set(*account, &p.url).map_err(bad_request)?;
    }
    Ok(Json(core.proxies.get(id).map_err(internal)?).into_response())
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct LogsQuery {
    after: u64,
    level: String,
    limit: usize,
}

async fn logs(_: AdminAuth, Query(q): Query<LogsQuery>) -> ApiResult {
    let level = if q.level.is_empty() { "INFO" } else { &q.level };
    let limit = if q.limit == 0 { 500 } else { q.limit };
    let lines = crate::logbuf::buffer().since(q.after, level, limit);
    Ok(Json(json!({"lines": lines})).into_response())
}

async fn clear_logs(_: AdminAuth) -> ApiResult {
    crate::logbuf::buffer().clear();
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn update_check(_: AdminAuth, State(core): State<Core>) -> ApiResult {
    if core.config.update_url.is_empty() {
        return Err(json_error(
            StatusCode::BAD_REQUEST,
            "update check is disabled (update_url is empty)",
        ));
    }
    let info = crate::update::check(&core.config.update_url)
        .await
        .map_err(|e| json_error(StatusCode::BAD_GATEWAY, format!("{e:#}")))?;
    Ok(Json(info).into_response())
}

async fn system(_: AdminAuth, State(core): State<Core>) -> ApiResult {
    let c = &core.config;
    Ok(Json(json!({
        "version": crate::VERSION,
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "data_dir": c.data_dir,
        "database": c.data_dir.join(crate::store::DB_FILE),
        "log_level": c.log_level,
        "update_check": !c.update_url.is_empty(),
        "desktop": core.service_control().is_some(),
    }))
    .into_response())
}
