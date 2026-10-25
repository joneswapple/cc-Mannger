//! The upstream Anthropic API client and the cached model catalog.

use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use bytes::Bytes;
use reqwest::Method;
use serde::{Deserialize, Serialize};

use crate::config::UpstreamConfig;
use crate::pool::{AccountPool, PickRequest};
use crate::provider::AccountProvider;

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
    /// One client per account proxy URL.
    proxied: Arc<std::sync::Mutex<std::collections::HashMap<String, reqwest::Client>>>,
}

impl Upstream {
    /// The client for an account: direct, or through its proxy only.
    fn client_for(&self, proxy_url: &str) -> Result<reqwest::Client> {
        if proxy_url.is_empty() {
            return Ok(self.http.clone());
        }
        let mut map = self.proxied.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(c) = map.get(proxy_url) {
            return Ok(c.clone());
        }
        let c = crate::oauth::proxied_client(proxy_url, None)?;
        map.insert(proxy_url.to_string(), c.clone());
        Ok(c)
    }

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
            proxied: Default::default(),
        }
    }

    pub fn source(&self) -> String {
        self.pool.source()
    }

    /// Sends a request, failing over across accounts.
    ///
    /// Each attempt picks an account (skipping ones already tried), sends,
    /// and classifies the status with [`classify`]. Retryable outcomes park
    /// or refresh the account and move on, up to `pool.max_retries` extra
    /// attempts. When attempts or accounts run out, the last upstream
    /// response is returned verbatim so clients still see Anthropic's own
    /// error. Bodies are never read here, so SSE streams are untouched.
    pub async fn forward(
        &self,
        req: &PickRequest,
        method: Method,
        path: &str,
        body: Option<Bytes>,
        extra: &ForwardExtra,
    ) -> Result<Forwarded, ForwardError> {
        let mut pick_req = req.clone();
        let mut failed: Vec<Attempt> = Vec::new();
        // Last retryable response, returned if nothing better comes along.
        let mut pending: Option<(reqwest::Response, Attempt)> = None;
        for _ in 0..=self.pool.config().max_retries {
            let provider = match self.pool.pick(&pick_req) {
                Ok(p) => p,
                Err(e) => return Err(finish_without_pick(pending, failed, e)),
            };
            if let Some((_, attempt)) = pending.take() {
                failed.push(attempt);
            }
            let account_id = provider.account_id();
            pick_req.exclude.push(account_id);
            let start = std::time::Instant::now();
            let result = self
                .send_once(&provider, method.clone(), path, body.clone(), extra)
                .await;
            let latency_ms = start.elapsed().as_millis() as i64;
            let resp = match result {
                Ok(resp) => resp,
                Err(e) => {
                    tracing::warn!(account = account_id, "upstream attempt failed: {e:#}");
                    failed.push(Attempt {
                        account_id,
                        status: 502,
                        error: format!("{e:#}"),
                        latency_ms,
                    });
                    continue;
                }
            };
            let snapshot = crate::quota::from_headers(resp.headers());
            if !snapshot.is_empty() {
                self.pool.record_quota(account_id, &snapshot);
            }
            let status = resp.status().as_u16();
            let verdict = classify(
                status,
                resp.headers(),
                self.pool.config().cooldown_secs as i64,
            );
            let error = match verdict {
                Verdict::Done => {
                    if status < 400 {
                        if let Err(e) = self.pool.store().mark_used(account_id) {
                            tracing::warn!(account = account_id, "mark used: {e:#}");
                        }
                    }
                    return Ok(Forwarded {
                        resp,
                        account_id,
                        failed,
                    });
                }
                Verdict::Retry {
                    cooldown: Some(secs),
                } => {
                    self.pool
                        .cool_down(account_id, secs, &format!("upstream {status}"));
                    format!("upstream {status}; account cooling down {secs}s")
                }
                Verdict::Retry { cooldown: None } => format!("upstream {status}"),
                Verdict::Reauth => {
                    provider.invalidate().await;
                    format!("upstream {status}; token refresh forced")
                }
            };
            pending = Some((
                resp,
                Attempt {
                    account_id,
                    status,
                    error,
                    latency_ms,
                },
            ));
        }
        match pending {
            Some((resp, attempt)) => Ok(Forwarded {
                resp,
                account_id: attempt.account_id,
                failed,
            }),
            None => {
                let last = failed.pop().expect("at least one attempt ran");
                Err(ForwardError::Exhausted { last, failed })
            }
        }
    }

    /// Polls the configured usage endpoint for every enabled account.
    pub async fn probe_usage(&self, usage_url: &str) {
        let accounts = match self.pool.store().list() {
            Ok(list) => list,
            Err(e) => return tracing::warn!("usage probe: {e:#}"),
        };
        for a in accounts.into_iter().filter(|a| a.enabled) {
            if let Err(e) = self.probe_one(usage_url, &a).await {
                tracing::debug!(account = a.id, "usage probe failed: {e:#}");
            }
        }
    }

    async fn probe_one(&self, usage_url: &str, a: &crate::accounts::Account) -> Result<()> {
        let provider = self.pool.provider_for(a.id)?;
        let token = provider.token().await?;
        let resp = self
            .client_for(&provider.proxy_url)?
            .get(usage_url)
            .bearer_auth(token)
            .header("anthropic-beta", "oauth-2025-04-20")
            .header("Accept", "application/json")
            .timeout(Duration::from_secs(30))
            .send()
            .await?;
        if !resp.status().is_success() {
            bail!("usage endpoint returned {}", resp.status());
        }
        let snap = crate::quota::from_usage_body(&resp.json().await?);
        self.pool.record_quota(a.id, &snap);
        Ok(())
    }

    async fn send_once(
        &self,
        provider: &AccountProvider,
        method: Method,
        path: &str,
        body: Option<Bytes>,
        extra: &ForwardExtra,
    ) -> Result<reqwest::Response> {
        let token = provider
            .token()
            .await
            .map_err(|e| e.context("acquire upstream token"))?;
        let mut req = self
            .client_for(&provider.proxy_url)?
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
        Ok(req.send().await?)
    }
}

/// When no account can be picked: hand back the last retryable response if
/// there is one, else the last failed attempt, else the pick error.
fn finish_without_pick(
    pending: Option<(reqwest::Response, Attempt)>,
    mut failed: Vec<Attempt>,
    pick_err: anyhow::Error,
) -> ForwardError {
    if let Some((resp, attempt)) = pending {
        return ForwardError::Response(Box::new(Forwarded {
            resp,
            account_id: attempt.account_id,
            failed,
        }));
    }
    match failed.pop() {
        Some(last) => ForwardError::Exhausted { last, failed },
        None => ForwardError::Pick(pick_err),
    }
}

/// One upstream attempt that did not produce the final answer.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Attempt {
    pub account_id: i64,
    /// HTTP status, or 502 for a transport / token failure.
    pub status: u16,
    pub error: String,
    pub latency_ms: i64,
}

/// The response to hand to the client, plus the attempts that failed over
/// before it.
pub struct Forwarded {
    pub resp: reqwest::Response,
    pub account_id: i64,
    pub failed: Vec<Attempt>,
}

impl std::fmt::Debug for Forwarded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Forwarded")
            .field("status", &self.resp.status())
            .field("account_id", &self.account_id)
            .field("failed", &self.failed)
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ForwardError {
    /// No account could be picked at all (`pool::NoAccount`,
    /// `pool::AllCooling`).
    #[error("{0:#}")]
    Pick(anyhow::Error),
    /// Every attempt failed without an upstream response.
    #[error("{}", last.error)]
    Exhausted { last: Attempt, failed: Vec<Attempt> },
    /// Accounts ran out after a retryable upstream response; forward it.
    #[error("upstream {}", .0.resp.status())]
    Response(Box<Forwarded>),
}

impl ForwardError {
    /// The response to forward when this is [`ForwardError::Response`].
    pub fn into_forwarded(self) -> Result<Forwarded, ForwardError> {
        match self {
            ForwardError::Response(f) => Ok(*f),
            e => Err(e),
        }
    }
}

/// What an upstream status means for failover.
#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Hand this response to the client (success, or a request error that
    /// another account wouldn't fix).
    Done,
    /// Try another account, optionally parking this one for `cooldown` s.
    Retry { cooldown: Option<i64> },
    /// The access token was rejected: force a refresh, try another account.
    Reauth,
}

/// 429 → park until the upstream's reset time (or `default_cooldown`);
/// 403 → park 30 minutes (account-level permission problem); 401 → refresh;
/// 500/502/503/504/529 → just try another account; anything else is final.
pub fn classify(
    status: u16,
    headers: &reqwest::header::HeaderMap,
    default_cooldown: i64,
) -> Verdict {
    match status {
        429 => Verdict::Retry {
            cooldown: Some(reset_after(headers, crate::now_unix()).unwrap_or(default_cooldown)),
        },
        403 => Verdict::Retry {
            cooldown: Some(30 * 60),
        },
        401 => Verdict::Reauth,
        500 | 502 | 503 | 504 | 529 => Verdict::Retry { cooldown: None },
        _ => Verdict::Done,
    }
}

/// Seconds until the upstream says the limit resets: `retry-after`
/// (seconds), else `anthropic-ratelimit-unified-reset`, else the earliest
/// future `anthropic-ratelimit-*-reset` (unix seconds or RFC 3339).
pub fn reset_after(headers: &reqwest::header::HeaderMap, now: i64) -> Option<i64> {
    let get = |k: &str| headers.get(k).and_then(|v| v.to_str().ok()).map(str::trim);
    if let Some(secs) = get("retry-after").and_then(|v| v.parse::<i64>().ok()) {
        return Some(secs.max(1));
    }
    let parse = |v: &str| -> Option<i64> {
        v.parse::<i64>()
            .ok()
            .or_else(|| {
                chrono::DateTime::parse_from_rfc3339(v)
                    .ok()
                    .map(|t| t.timestamp())
            })
            .map(|t| t - now)
            .filter(|s| *s > 0)
    };
    if let Some(s) = get("anthropic-ratelimit-unified-reset").and_then(parse) {
        return Some(s);
    }
    headers
        .iter()
        .filter(|(k, _)| {
            let k = k.as_str();
            k.starts_with("anthropic-ratelimit-") && k.ends_with("-reset")
        })
        .filter_map(|(_, v)| v.to_str().ok().and_then(|v| parse(v.trim())))
        .min()
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
        let resp = self
            .upstream
            .forward(
                &PickRequest::default(),
                Method::GET,
                "/v1/models?limit=1000",
                None,
                &ForwardExtra::default(),
            )
            .await
            .or_else(ForwardError::into_forwarded)?
            .resp;
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

    fn headers(pairs: &[(&'static str, &str)]) -> reqwest::header::HeaderMap {
        let mut h = reqwest::header::HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, v.parse().unwrap());
        }
        h
    }

    #[test]
    fn classify_statuses() {
        let none = headers(&[]);
        assert_eq!(classify(200, &none, 60), Verdict::Done);
        assert_eq!(classify(400, &none, 60), Verdict::Done);
        assert_eq!(classify(404, &none, 60), Verdict::Done);
        assert_eq!(
            classify(429, &none, 60),
            Verdict::Retry { cooldown: Some(60) }
        );
        assert_eq!(
            classify(429, &headers(&[("retry-after", "17")]), 60),
            Verdict::Retry { cooldown: Some(17) }
        );
        assert_eq!(
            classify(403, &none, 60),
            Verdict::Retry {
                cooldown: Some(1800)
            }
        );
        assert_eq!(classify(401, &none, 60), Verdict::Reauth);
        for s in [500, 502, 503, 504, 529] {
            assert_eq!(classify(s, &none, 60), Verdict::Retry { cooldown: None });
        }
    }

    #[test]
    fn reset_after_sources() {
        let now = 1_000_000;
        assert_eq!(reset_after(&headers(&[("retry-after", "0")]), now), Some(1));
        assert_eq!(
            reset_after(
                &headers(&[("anthropic-ratelimit-unified-reset", "1000300")]),
                now
            ),
            Some(300)
        );
        // Earliest future reset among the other limit headers; past ones ignored.
        let h = headers(&[
            ("anthropic-ratelimit-tokens-reset", "1970-01-12T13:53:20Z"),
            ("anthropic-ratelimit-requests-reset", "1000050"),
            ("anthropic-ratelimit-input-tokens-reset", "999000"),
        ]);
        assert_eq!(reset_after(&h, now), Some(50));
        assert_eq!(
            reset_after(
                &headers(&[("retry-after", "Wed, 21 Oct 2015 07:28:00 GMT")]),
                now
            ),
            None
        );
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
