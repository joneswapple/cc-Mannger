//! Configuration: code defaults → `config.json` → `CCM_*` environment
//! variables, in that order of precedence.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub listen: String,
    pub data_dir: PathBuf,
    pub log_level: String,
    /// Base64-encoded 32-byte key sealing account tokens. Empty means use
    /// (or create) `data_dir/secret.key`.
    pub secret_key: String,
    pub upstream: UpstreamConfig,
    pub pool: PoolConfig,
    pub routing: RoutingConfig,
    pub admin: AdminConfig,
    pub monitor: MonitorConfig,
    pub security: SecurityConfig,
    pub desktop: DesktopConfig,
    pub tunnel: TunnelConfig,
    /// Where `cloudflared` is downloaded from; empty means the official
    /// GitHub release for this platform.
    pub cloudflared_download_url: String,
    /// Claude Code settings file written by "sync to this machine"; empty
    /// means `~/.claude/settings.json`.
    pub claude_settings_path: String,
    /// GitHub "latest release" API URL for the update check; empty
    /// disables it.
    pub update_url: String,
}

/// Public access through a Cloudflare Tunnel (`tunnel.rs`). The token for
/// token mode is stored encrypted, not here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TunnelConfig {
    /// `quick` (temporary *.trycloudflare.com URL, no account) or `token`
    /// (a named tunnel created in the Cloudflare dashboard).
    pub mode: TunnelMode,
    /// Start the tunnel together with the server / app.
    pub auto_start: bool,
    /// Also serve the admin API (and web UI) through the tunnel. Off by
    /// default: only `/v1/*`, `/register` and `/healthz` are public.
    pub expose_admin: bool,
    /// `auto`, `http2` or `quic` (`--protocol`); http2 helps where UDP is
    /// blocked.
    pub protocol: String,
    /// Loopback port of the tunnel's own listener. In token mode, point the
    /// tunnel's public hostname at `http://localhost:<port>`.
    pub local_port: u16,
    /// Use this `cloudflared` instead of the managed or PATH one.
    pub binary_path: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TunnelMode {
    #[default]
    Quick,
    Token,
}

impl Default for TunnelConfig {
    fn default() -> Self {
        Self {
            mode: TunnelMode::Quick,
            auto_start: false,
            expose_admin: false,
            protocol: "auto".into(),
            local_port: 8788,
            binary_path: String::new(),
        }
    }
}

impl TunnelConfig {
    pub fn validate(&self) -> Result<()> {
        if !matches!(self.protocol.as_str(), "auto" | "http2" | "quic") {
            anyhow::bail!("tunnel.protocol must be auto, http2 or quic");
        }
        if self.local_port == 0 {
            anyhow::bail!("tunnel.local_port must be a fixed port");
        }
        Ok(())
    }
}

/// Client-IP handling for `/v1/*` and `/register` (`security.rs`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SecurityConfig {
    /// Message returned to a blocked IP.
    pub block_message: String,
    /// Take the client IP from `X-Forwarded-For` / `X-Real-IP`. Only safe
    /// behind a reverse proxy that sets them; otherwise clients can spoof.
    pub trust_forwarded_for: bool,
    /// Days of the IP access log to keep.
    pub access_log_days: i64,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            block_message: "access from this IP address is not allowed".into(),
            trust_forwarded_for: false,
            access_log_days: 7,
        }
    }
}

impl SecurityConfig {
    pub fn validate(&self) -> Result<()> {
        if !(1..=365).contains(&self.access_log_days) {
            anyhow::bail!("security.access_log_days must be between 1 and 365");
        }
        Ok(())
    }
}

/// The request monitor (`monitor.rs`): recent requests with their bodies,
/// kept in memory only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MonitorConfig {
    /// Off by default: recorded bodies contain prompts and completions.
    pub enabled: bool,
    /// Requests kept; the oldest is dropped first.
    pub capacity: usize,
    /// Bytes kept per body (request, upstream request, response).
    pub max_body_bytes: usize,
}

impl Default for MonitorConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            capacity: 500,
            max_body_bytes: 256 << 10,
        }
    }
}

/// Desktop-shell behaviour.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DesktopConfig {
    /// Start the proxy listener when the app starts.
    pub auto_start_proxy: bool,
}

impl Default for DesktopConfig {
    fn default() -> Self {
        Self {
            auto_start_proxy: true,
        }
    }
}

/// Model rewriting beyond the editable route list (see `routing.rs`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RoutingConfig {
    /// Send small, tool-less requests (titles, summaries) to this model;
    /// empty disables the rule.
    pub background_model: String,
    /// "Small" means `max_tokens` at or below this.
    pub background_max_tokens: i64,
}

impl Default for RoutingConfig {
    fn default() -> Self {
        Self {
            background_model: String::new(),
            background_max_tokens: 512,
        }
    }
}

/// How requests are spread over upstream accounts and retried.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PoolConfig {
    /// `priority` (highest priority first), `round_robin` (weighted by
    /// `weight`), or `least_used` (fewest tokens in the last 5 hours).
    pub strategy: Strategy,
    /// Keep a session (`metadata.user_id`, else the terminal key) on the
    /// same account while it stays healthy, so prompt caches keep hitting.
    pub sticky: bool,
    /// Forget an idle sticky binding after this many seconds.
    pub sticky_ttl_secs: u64,
    /// Extra accounts to try after a retryable upstream failure.
    pub max_retries: u32,
    /// Cooldown after a 429 when the upstream gives no reset time.
    pub cooldown_secs: u64,
    /// Accounts whose 5h or 7d usage is at or above this fraction are only
    /// used when no account below it is available.
    pub quota_soft_limit: f64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Strategy {
    #[default]
    Priority,
    RoundRobin,
    LeastUsed,
}

impl std::str::FromStr for Strategy {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "priority" => Ok(Self::Priority),
            "round_robin" => Ok(Self::RoundRobin),
            "least_used" => Ok(Self::LeastUsed),
            _ => Err(format!(
                "unknown pool strategy {s:?} (priority | round_robin | least_used)"
            )),
        }
    }
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            strategy: Strategy::Priority,
            sticky: true,
            sticky_ttl_secs: 3600,
            max_retries: 2,
            cooldown_secs: 60,
            quota_soft_limit: 0.95,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UpstreamConfig {
    pub base_url: String,
    /// Claude Code CLI credentials file imported as account #1 on first
    /// start. Empty means auto-detect (see `oauth::resolve_credentials_path`).
    pub credentials_path: String,
    /// The OAuth token endpoint and client id are not officially documented;
    /// the defaults match the public Claude Code CLI.
    pub oauth_token_url: String,
    pub oauth_client_id: String,
    /// PKCE login: where the browser is sent, where it comes back (the
    /// default console page shows a `code#state` to paste), and the scopes.
    pub oauth_authorize_url: String,
    pub oauth_redirect_uri: String,
    pub oauth_scopes: String,
    /// Account profile (email / organization / plan) for a bearer token.
    pub profile_url: String,
    /// Optional usage endpoint polled for each account's 5h / 7d usage
    /// (`{"five_hour":{"utilization","resets_at"},"seven_day":{…}}`).
    /// Empty disables polling; usage is still read from response headers.
    pub usage_url: String,
    pub usage_poll_secs: u64,
    pub anthropic_beta: String,
    pub user_agent: String,
    /// Prepend the literal Claude Code system block when missing — required
    /// for OAuth Pro/Max bearer auth.
    pub inject_claude_code_system: bool,
    /// Keep the imported credentials file authoritative (re-read before
    /// refreshes, rewritten after) so the host's `claude` CLI and CC Manager
    /// don't invalidate each other's refresh token.
    pub sync_credentials_file: bool,
    /// OpenAI-style effort tiers (`"effort":"high"` in a request body)
    /// expanded into Anthropic `max_tokens` + extended-thinking budget.
    pub effort_levels: BTreeMap<String, EffortLevel>,
}

/// One effort tier. Applied only where the request doesn't already set
/// `max_tokens` / `thinking`; explicit values always win.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EffortLevel {
    pub max_tokens: i64,
    /// When > 0, also enables extended thinking with this `budget_tokens`.
    pub thinking_budget: i64,
}

pub fn default_effort_levels() -> BTreeMap<String, EffortLevel> {
    [
        ("auto", 8192, 0),
        ("low", 4096, 0),
        ("medium", 16384, 5000),
        ("high", 32768, 15000),
        ("xhigh", 48000, 30000),
        ("max", 64000, 60000),
    ]
    .into_iter()
    .map(|(name, max_tokens, thinking_budget)| {
        (
            name.to_string(),
            EffortLevel {
                max_tokens,
                thinking_budget,
            },
        )
    })
    .collect()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AdminConfig {
    /// Fixed admin token. Empty means generate one on first start.
    pub initial_token: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen: "0.0.0.0:8787".into(),
            data_dir: default_data_dir(),
            log_level: "info".into(),
            secret_key: String::new(),
            upstream: UpstreamConfig::default(),
            pool: PoolConfig::default(),
            routing: RoutingConfig::default(),
            admin: AdminConfig::default(),
            monitor: MonitorConfig::default(),
            security: SecurityConfig::default(),
            tunnel: TunnelConfig::default(),
            cloudflared_download_url: String::new(),
            desktop: DesktopConfig::default(),
            claude_settings_path: String::new(),
            update_url: "https://api.github.com/repos/joneswapple/cc-Mannger/releases/latest"
                .into(),
        }
    }
}

impl Default for UpstreamConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.anthropic.com".into(),
            credentials_path: String::new(),
            oauth_token_url: "https://console.anthropic.com/v1/oauth/token".into(),
            oauth_client_id: "9d1c250a-e61b-44d9-88ed-5944d1962f5e".into(),
            oauth_authorize_url: "https://claude.ai/oauth/authorize".into(),
            oauth_redirect_uri: "https://console.anthropic.com/oauth/code/callback".into(),
            oauth_scopes: "org:create_api_key user:profile user:inference".into(),
            profile_url: "https://api.anthropic.com/api/oauth/profile".into(),
            usage_url: String::new(),
            usage_poll_secs: 600,
            // Both betas are required for OAuth-bearer /v1/messages calls.
            anthropic_beta: "claude-code-20250219,oauth-2025-04-20".into(),
            user_agent: "claude-cli/1.0 (cc-manager)".into(),
            inject_claude_code_system: true,
            sync_credentials_file: true,
            effort_levels: default_effort_levels(),
        }
    }
}

/// `<platform data dir>/cc-manager`, e.g. `~/.local/share/cc-manager`.
pub fn default_data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("cc-manager")
}

impl Config {
    /// Loads `path` if given and present, then applies `CCM_*` overrides.
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let mut cfg = match path {
            Some(p) if p.exists() => {
                let b = std::fs::read(p).with_context(|| format!("read {}", p.display()))?;
                serde_json::from_slice(&b).with_context(|| format!("parse {}", p.display()))?
            }
            _ => Config::default(),
        };
        cfg.apply_env(|k| std::env::var(k).ok());
        cfg.data_dir = expand_home(&cfg.data_dir);
        cfg.upstream.credentials_path = expand_home(Path::new(&cfg.upstream.credentials_path))
            .to_string_lossy()
            .into_owned();
        Ok(cfg)
    }

    fn apply_env(&mut self, get: impl Fn(&str) -> Option<String>) {
        let set = |dst: &mut String, key: &str| {
            if let Some(v) = get(key).filter(|v| !v.is_empty()) {
                *dst = v;
            }
        };
        set(&mut self.listen, "CCM_LISTEN");
        set(&mut self.log_level, "CCM_LOG_LEVEL");
        set(&mut self.secret_key, "CCM_SECRET_KEY");
        set(&mut self.upstream.base_url, "CCM_UPSTREAM_BASE_URL");
        set(&mut self.upstream.credentials_path, "CCM_CREDENTIALS_PATH");
        set(&mut self.admin.initial_token, "CCM_ADMIN_TOKEN");
        set(&mut self.claude_settings_path, "CCM_CLAUDE_SETTINGS");
        if let Some(v) = get("CCM_POOL_STRATEGY").filter(|v| !v.is_empty()) {
            match v.parse() {
                Ok(s) => self.pool.strategy = s,
                Err(e) => tracing::warn!("CCM_POOL_STRATEGY ignored: {e}"),
            }
        }
        if let Some(v) = get("CCM_POOL_STICKY").filter(|v| !v.is_empty()) {
            self.pool.sticky = v == "1" || v.eq_ignore_ascii_case("true");
        }
        if let Some(v) = get("CCM_POOL_MAX_RETRIES").and_then(|v| v.parse().ok()) {
            self.pool.max_retries = v;
        }
        if let Some(v) = get("CCM_DATA_DIR").filter(|v| !v.is_empty()) {
            self.data_dir = PathBuf::from(v);
        }
        if let Some(v) = get("CCM_SYNC_CREDENTIALS_FILE").filter(|v| !v.is_empty()) {
            self.upstream.sync_credentials_file = v == "1" || v.eq_ignore_ascii_case("true");
        }
    }
}

fn expand_home(p: &Path) -> PathBuf {
    match (p.strip_prefix("~"), dirs::home_dir()) {
        (Ok(rest), Some(home)) => home.join(rest),
        _ => p.to_path_buf(),
    }
}

/// Pretty JSON, as written to `config.json`.
pub fn to_pretty_json(cfg: &Config) -> serde_json::Result<String> {
    serde_json::to_string_pretty(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_then_env_precedence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{"listen":"127.0.0.1:1","upstream":{"base_url":"http://file"}}"#,
        )
        .unwrap();
        let mut cfg: Config = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(cfg.listen, "127.0.0.1:1");
        assert_eq!(cfg.upstream.base_url, "http://file");
        // Unset fields keep their defaults.
        assert!(cfg.upstream.sync_credentials_file);
        cfg.apply_env(|k| match k {
            "CCM_LISTEN" => Some("127.0.0.1:2".into()),
            "CCM_SYNC_CREDENTIALS_FILE" => Some("false".into()),
            "CCM_POOL_STRATEGY" => Some("least_used".into()),
            _ => None,
        });
        assert_eq!(cfg.listen, "127.0.0.1:2");
        assert!(!cfg.upstream.sync_credentials_file);
        assert_eq!(cfg.pool.strategy, Strategy::LeastUsed);
    }
}
