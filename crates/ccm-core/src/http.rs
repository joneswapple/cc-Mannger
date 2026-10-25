//! The HTTP server. R0 only exposes service status; the Anthropic proxy and
//! admin API arrive in R1.

use std::future::Future;

use anyhow::{Context, Result};
use axum::{extract::State, routing::get, Json, Router};
use serde::Serialize;

use crate::Core;

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub version: &'static str,
    pub listen: String,
    pub accounts: i64,
    pub upstream: String,
}

impl Core {
    pub fn status(&self) -> Result<Status> {
        Ok(Status {
            version: crate::VERSION,
            listen: self.config.listen.clone(),
            accounts: self.accounts.count()?,
            upstream: self.pool.source(),
        })
    }
}

pub fn router(core: Core) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .with_state(core)
}

async fn healthz(
    State(core): State<Core>,
) -> Result<Json<Status>, (axum::http::StatusCode, String)> {
    core.status().map(Json).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("{e:#}"),
        )
    })
}

/// Binds `core.config.listen` and serves until `shutdown` resolves.
pub async fn serve(core: Core, shutdown: impl Future<Output = ()> + Send + 'static) -> Result<()> {
    let listen = core.config.listen.clone();
    let listener = tokio::net::TcpListener::bind(&listen)
        .await
        .with_context(|| format!("listen on {listen}"))?;
    tracing::info!("cc-manager listening on {listen}");
    axum::serve(listener, router(core))
        .with_graceful_shutdown(shutdown)
        .await?;
    Ok(())
}
