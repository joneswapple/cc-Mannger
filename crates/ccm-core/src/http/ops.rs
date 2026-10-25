//! Operational admin endpoints: runtime settings, the proxy listener, the
//! request monitor, model connectivity tests, and syncing this machine's
//! Claude Code settings.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use super::admin::{blocking, internal, ApiResult};
use super::{json_error, proxy, AdminAuth};
use crate::claude_code::ClaudeCodeSettings;
use crate::monitor::Filter;
use crate::Core;

/// Terminal user that owns the key written into this machine's Claude
/// Code settings.
pub const LOCAL_CLAUDE_CODE_USER: &str = "claude-code (this machine)";

pub(super) fn routes() -> Router<Core> {
    Router::new()
        .route("/runtime", get(get_runtime).post(update_runtime))
        .route("/service", get(service_status))
        .route("/service/:action", post(service_action))
        .route("/monitor", get(monitor_list))
        .route("/monitor/clear", post(monitor_clear))
        .route("/monitor/:id", get(monitor_get))
        .route("/proxy/test", post(test_model))
        .route("/claude-code", get(claude_code_status))
        .route("/claude-code/sync", post(claude_code_sync))
        .route("/claude-code/restore", post(claude_code_restore))
}

fn bad_request(e: anyhow::Error) -> Response {
    json_error(StatusCode::BAD_REQUEST, format!("{e:#}"))
}

fn runtime_view(core: &Core) -> anyhow::Result<Value> {
    let effective = core.runtime.get();
    let bound = core.listening();
    Ok(json!({
        "effective": effective,
        "overrides": core.runtime.overrides()?,
        "base": core.runtime.base(),
        "restart_required": bound.as_deref().is_some_and(|b| !same_listen(b, &effective.listen)),
    }))
}

/// `0.0.0.0:8787` and `0.0.0.0:8787` match; a bound `127.0.0.1:0` (tests)
/// never asks for a restart.
fn same_listen(bound: &str, wanted: &str) -> bool {
    bound == wanted || wanted.ends_with(":0")
}

async fn get_runtime(_: AdminAuth, State(core): State<Core>) -> ApiResult {
    Ok(Json(runtime_view(&core).map_err(internal)?).into_response())
}

/// Body: a JSON merge patch over the settings (`null` resets a field).
async fn update_runtime(
    _: AdminAuth,
    State(core): State<Core>,
    Json(patch): Json<Value>,
) -> ApiResult {
    core.update_runtime(&patch).map_err(bad_request)?;
    tracing::info!("runtime settings updated");
    Ok(Json(runtime_view(&core).map_err(internal)?).into_response())
}

fn service_view(core: &Core) -> Value {
    let listen = core.runtime.get().listen;
    let bound = core.listening();
    json!({
        "running": bound.is_some(),
        "bound": bound,
        "listen": listen,
        "restart_required": bound.as_deref().is_some_and(|b| !same_listen(b, &listen)),
        "controllable": core.service_control().is_some(),
    })
}

async fn service_status(_: AdminAuth, State(core): State<Core>) -> ApiResult {
    Ok(Json(service_view(&core)).into_response())
}

async fn service_action(
    _: AdminAuth,
    State(core): State<Core>,
    Path(action): Path<String>,
) -> ApiResult {
    let Some(ctl) = core.service_control() else {
        return Err(json_error(
            StatusCode::BAD_REQUEST,
            "this server cannot stop or restart its own listener; restart ccm-server instead",
        ));
    };
    let r = match action.as_str() {
        "start" => ctl.start(),
        "stop" => ctl.stop(),
        "restart" => ctl.restart(),
        _ => return Err(json_error(StatusCode::NOT_FOUND, "unknown action")),
    };
    r.map_err(bad_request)?;
    // Let the listener settle so the reply reflects the new state.
    for _ in 0..20 {
        let running = core.listening().is_some();
        if (action == "stop") != running {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    Ok(Json(service_view(&core)).into_response())
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct MonitorQuery {
    after_id: u64,
    q: String,
    status: String,
    account_id: Option<i64>,
    limit: usize,
}

async fn monitor_list(
    _: AdminAuth,
    State(core): State<Core>,
    Query(q): Query<MonitorQuery>,
) -> ApiResult {
    let (items, held) = core.monitor.list(&Filter {
        after_id: q.after_id,
        q: q.q,
        status: q.status,
        account_id: q.account_id,
        limit: q.limit,
    });
    let cfg = core.monitor.config();
    Ok(Json(json!({
        "enabled": cfg.enabled,
        "capacity": cfg.capacity,
        "max_body_bytes": cfg.max_body_bytes,
        "held": held,
        "items": items,
    }))
    .into_response())
}

async fn monitor_get(_: AdminAuth, State(core): State<Core>, Path(id): Path<u64>) -> ApiResult {
    match core.monitor.get(id) {
        Some(l) => Ok(Json(l).into_response()),
        None => Err(json_error(
            StatusCode::NOT_FOUND,
            "request not in the monitor",
        )),
    }
}

async fn monitor_clear(_: AdminAuth, State(core): State<Core>) -> ApiResult {
    core.monitor.clear();
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(Deserialize)]
struct TestRequest {
    model: String,
}

async fn test_model(
    _: AdminAuth,
    State(core): State<Core>,
    Json(req): Json<TestRequest>,
) -> ApiResult {
    if req.model.trim().is_empty() {
        return Err(json_error(StatusCode::BAD_REQUEST, "model required"));
    }
    Ok(Json(proxy::probe_model(&core, req.model.trim()).await).into_response())
}

fn claude_settings(core: &Core) -> anyhow::Result<ClaudeCodeSettings> {
    ClaudeCodeSettings::new(&core.config.claude_settings_path)
}

async fn claude_code_status(_: AdminAuth, State(core): State<Core>) -> ApiResult {
    let s = claude_settings(&core).map_err(internal)?;
    Ok(Json(s.status().map_err(bad_request)?).into_response())
}

#[derive(Deserialize)]
struct SyncRequest {
    /// The URL this machine's Claude Code should use, e.g.
    /// `http://127.0.0.1:8787`.
    base_url: String,
}

/// Issues a fresh key for the local Claude Code terminal, writes it with
/// the base URL into the settings file, then revokes that terminal's older
/// keys. The key itself is never returned.
async fn claude_code_sync(
    _: AdminAuth,
    State(core): State<Core>,
    Json(req): Json<SyncRequest>,
) -> ApiResult {
    let settings = claude_settings(&core).map_err(internal)?;
    let keys = core.keys.clone();
    let base_url = req.base_url.trim().to_string();
    let status = blocking(move || {
        let users = keys.list_users()?;
        let (user_id, old) = match users.iter().find(|u| u.label == LOCAL_CLAUDE_CODE_USER) {
            Some(u) => (
                u.id,
                u.keys
                    .iter()
                    .filter(|k| k.revoked_at.is_none())
                    .map(|k| k.id)
                    .collect::<Vec<_>>(),
            ),
            None => (keys.create_user(LOCAL_CLAUDE_CODE_USER)?, vec![]),
        };
        let (key, key_id) = keys.issue(user_id)?;
        let status = match settings.apply(&base_url, &key) {
            Ok(s) => s,
            Err(e) => {
                keys.revoke(key_id)?;
                return Err(e);
            }
        };
        for id in old {
            keys.revoke(id)?;
        }
        Ok(status)
    })
    .await
    .map_err(bad_request)?;
    tracing::info!("claude code settings synced to {}", status.path);
    Ok(Json(status).into_response())
}

/// Puts the original settings file back and revokes the keys that were
/// written for this machine.
async fn claude_code_restore(_: AdminAuth, State(core): State<Core>) -> ApiResult {
    let s = claude_settings(&core).map_err(internal)?;
    let status = s.restore().map_err(bad_request)?;
    let keys = core.keys.clone();
    blocking(move || {
        for u in keys.list_users()? {
            if u.label == LOCAL_CLAUDE_CODE_USER {
                for k in u.keys.iter().filter(|k| k.revoked_at.is_none()) {
                    keys.revoke(k.id)?;
                }
            }
        }
        Ok(())
    })
    .await
    .map_err(internal)?;
    Ok(Json(status).into_response())
}
