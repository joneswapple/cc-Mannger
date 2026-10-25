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
    pub admin: AdminConfig,
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
            admin: AdminConfig::default(),
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
            _ => None,
        });
        assert_eq!(cfg.listen, "127.0.0.1:2");
        assert!(!cfg.upstream.sync_credentials_file);
    }
}
