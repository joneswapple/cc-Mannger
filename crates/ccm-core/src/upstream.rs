//! The upstream Anthropic API client and the cached model catalog.

use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use bytes::Bytes;
use reqwest::Method;
use serde::{Deserialize, Serialize};

use crate::config::UpstreamConfig;
use crate::provider::AccountPool;

/// Per-request hints from the inbound caller, layered over the configured
/// defaults. All optional.
#[derive(Debug, Default, Clone)]
pub struct ForwardExtra {
    /// Merged into `anthropic-beta` after the configured flags.
    pub extra_beta: Option<String>,
    /// Replaces the configured User-Agent; see [`passthrough_user_agent`].
    pub user_agent: Option<String>,
    pub content_type: Option<String>,
}

#[derive(Clone)]
pub struct Upstream {
    http: reqwest::Client,
    base_url: String,
    user_agent: String,
    beta: String,
    pool: AccountPool,
}

impl Upstream {
    pub fn new(cfg: &UpstreamConfig, pool: AccountPool) -> Self {
        Self {
            // No overall timeout: responses stream for minutes. Cancellation
            // comes from the client disconnecting.
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(30))
                .pool_idle_timeout(Duration::from_secs(90))
                .build()
                .expect("build upstream client"),
            base_url: cfg.base_url.trim_end_matches('/').to_string(),
            user_agent: cfg.user_agent.clone(),
            beta: cfg.anthropic_beta.clone(),
            pool,
        }
    }

    pub fn source(&self) -> String {
        self.pool.source()
    }

    /// Sends a request with the picked account's bearer token. Returns the
    /// response and the id of the account that served it.
    pub async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<Bytes>,
        extra: &ForwardExtra,
    ) -> Result<(reqwest::Response, i64)> {
        let provider = self.pool.pick()?;
        let account_id = provider.account_id();
        let token = provider
            .token()
            .await
            .map_err(|e| e.context("acquire upstream token"))
            .map_err(|e| UpstreamError {
                account_id,
                inner: e,
            })?;
        let mut req = self
            .http
            .request(method, format!("{}{path}", self.base_url))
            .bearer_auth(token)
            .header("anthropic-version", "2023-06-01")
            .header("Accept", "application/json")
            .header(
                "User-Agent",
                extra.user_agent.as_deref().unwrap_or(&self.user_agent),
            );
        let beta = merge_beta(&self.beta, extra.extra_beta.as_deref().unwrap_or(""));
        if !beta.is_empty() {
            req = req.header("anthropic-beta", beta);
        }
        if let Some(body) = body {
            req = req
                .header(
                    "Content-Type",
                    extra.content_type.as_deref().unwrap_or("application/json"),
                )
                .body(body);
        }
        let resp = req.send().await.map_err(|e| UpstreamError {
            account_id,
            inner: e.into(),
        })?;
        Ok((resp, account_id))
    }
}

/// An upstream failure after an account was picked, so callers can still
/// attribute it.
#[derive(Debug, thiserror::Error)]
#[error("{inner:#}")]
pub struct UpstreamError {
    pub account_id: i64,
    pub inner: anyhow::Error,
}

/// Combines two comma-separated `anthropic-beta` values: `primary` order
/// first, new flags from `extra` appended, whitespace trimmed, duplicates
/// dropped.
pub fn merge_beta(primary: &str, extra: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for flag in primary.split(',').chain(extra.split(',')) {
        let flag = flag.trim();
        if !flag.is_empty() && !out.contains(&flag) {
            out.push(flag);
        }
    }
    out.join(",")
}

/// The inbound User-Agent, only when it looks like an official Anthropic
/// client. Generic UAs (curl, SDKs) are replaced by the configured default,
/// which mimics Claude Code as Anthropic's anti-abuse layer expects.
pub fn passthrough_user_agent(ua: &str) -> Option<String> {
    let low = ua.trim().to_ascii_lowercase();
    ["claude-cli/", "claude-code/", "anthropic-", "claude/"]
        .iter()
        .any(|p| low.starts_with(p))
        .then(|| ua.to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct CatalogStatus {
    pub models: usize,
    pub last_fetched: Option<i64>,
    pub last_error: Option<String>,
}

#[derive(Default)]
struct CatalogState {
    models: Vec<Model>,
    last_fetched: Option<i64>,
    last_error: Option<String>,
}

/// The upstream model list, refreshed periodically. A failed refresh keeps
/// the previous list.
#[derive(Clone)]
pub struct Catalog {
    upstream: Upstream,
    state: Arc<RwLock<CatalogState>>,
}

#[derive(Deserialize)]
struct ModelsPage {
    #[serde(default)]
    data: Vec<Model>,
}

impl Catalog {
    pub fn new(upstream: Upstream) -> Self {
        Self {
            upstream,
            state: Default::default(),
        }
    }

    /// Initial fetch, then a refresh every `every` (none when zero). Must be
    /// called inside a Tokio runtime.
    pub fn start(&self, every: Duration) {
        let me = self.clone();
        tokio::spawn(async move {
            match me.refresh().await {
                Ok(n) => tracing::info!("model catalog: {n} model(s) loaded from upstream"),
                Err(e) => tracing::warn!("model catalog: initial fetch failed: {e:#} — if every request also fails with an auth error, log the account in again"),
            }
            if every.is_zero() {
                return;
            }
            let mut tick = tokio::time::interval(every);
            tick.tick().await;
            loop {
                tick.tick().await;
                if let Err(e) = me.refresh().await {
                    tracing::warn!("model catalog: refresh failed: {e:#}");
                }
            }
        });
    }

    pub async fn refresh(&self) -> Result<usize> {
        let result = self.fetch().await;
        let mut st = self.state.write().unwrap_or_else(|e| e.into_inner());
        match result {
            Ok(models) => {
                let n = models.len();
                st.models = models;
                st.last_fetched = Some(crate::now_unix());
                st.last_error = None;
                Ok(n)
            }
            Err(e) => {
                st.last_error = Some(format!("{e:#}"));
                Err(e)
            }
        }
    }

    async fn fetch(&self) -> Result<Vec<Model>> {
        let (resp, _) = self
            .upstream
            .send(
                Method::GET,
                "/v1/models?limit=1000",
                None,
                &ForwardExtra::default(),
            )
            .await?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!(
                "upstream /v1/models returned {}: {}",
                status.as_u16(),
                crate::oauth::truncate(&body, 200)
            );
        }
        let mut page: ModelsPage = serde_json::from_str(&body).context("decode models response")?;
        if page.data.is_empty() {
            bail!("upstream /v1/models returned no models for this account/plan");
        }
        page.data.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(page.data)
    }

    pub fn models(&self) -> Vec<Model> {
        self.state
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .models
            .clone()
    }

    pub fn ids(&self) -> Vec<String> {
        self.models().into_iter().map(|m| m.id).collect()
    }

    pub fn status(&self) -> CatalogStatus {
        let st = self.state.read().unwrap_or_else(|e| e.into_inner());
        CatalogStatus {
            models: st.models.len(),
            last_fetched: st.last_fetched,
            last_error: st.last_error.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_beta_dedups_and_keeps_order() {
        assert_eq!(merge_beta("a,b", " b , c,,a"), "a,b,c");
        assert_eq!(merge_beta("", "x"), "x");
        assert_eq!(merge_beta("", ""), "");
    }

    #[test]
    fn user_agent_passthrough() {
        assert_eq!(
            passthrough_user_agent("claude-cli/2.0 (linux)").as_deref(),
            Some("claude-cli/2.0 (linux)")
        );
        assert_eq!(
            passthrough_user_agent("Anthropic-SDK/1"),
            Some("Anthropic-SDK/1".into())
        );
        assert_eq!(passthrough_user_agent("curl/8.4.0"), None);
        assert_eq!(passthrough_user_agent(""), None);
    }
}
