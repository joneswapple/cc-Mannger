//! Claude Code CLI credentials files and the OAuth refresh call.
//!
//! Credentials file shapes accepted:
//!
//! 1. `{"claudeAiOauth": {"accessToken", "refreshToken", "expiresAt" (ms), "scopes"}}`
//!    — the current Claude Code CLI.
//! 2. The same fields at the top level (older builds).
//! 3. Top-level snake_case `access_token` / `refresh_token` / `expires_at`
//!    (ms or s) / `expires_in` (OAuth2 standard, community tools).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::accounts::Credentials;

#[derive(Deserialize, Serialize)]
struct CredsFile {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: Credentials,
}

#[derive(Deserialize)]
struct SnakeCreds {
    access_token: String,
    #[serde(default)]
    refresh_token: String,
    #[serde(default)]
    expires_at: i64,
    #[serde(default)]
    expires_in: i64,
    #[serde(default)]
    scopes: Vec<String>,
}

/// Parses any accepted credentials file shape into a normalized token set.
pub fn parse_credentials(b: &[u8]) -> Result<Credentials> {
    if let Ok(f) = serde_json::from_slice::<CredsFile>(b) {
        if !f.claude_ai_oauth.access_token.is_empty() {
            return Ok(f.claude_ai_oauth);
        }
    }
    if let Ok(c) = serde_json::from_slice::<Credentials>(b) {
        if !c.access_token.is_empty() {
            return Ok(c);
        }
    }
    if let Ok(s) = serde_json::from_slice::<SnakeCreds>(b) {
        if !s.access_token.is_empty() {
            let mut expires_at = s.expires_at;
            // Values below 10^12 are unix seconds, not millis.
            if expires_at > 0 && expires_at < 1_000_000_000_000 {
                expires_at *= 1000;
            }
            if expires_at == 0 && s.expires_in > 0 {
                expires_at = crate::now_millis() + s.expires_in * 1000;
            }
            return Ok(Credentials {
                access_token: s.access_token,
                refresh_token: s.refresh_token,
                expires_at,
                scopes: s.scopes,
            });
        }
    }
    bail!("credentials file did not contain a recognised access token (expected `claudeAiOauth.accessToken`, `accessToken`, or `access_token`)")
}

pub fn read_credentials_file(path: &Path) -> Result<Credentials> {
    let b = std::fs::read(path).with_context(|| format!("read credentials {}", path.display()))?;
    parse_credentials(&b)
}

/// Finds the Claude Code CLI credentials file: the explicit path if given,
/// else `$CLAUDE_CREDENTIALS_PATH`, `$CLAUDE_CONFIG_DIR/.credentials.json`,
/// then `~/.claude/.credentials.json`.
pub fn resolve_credentials_path(explicit: &str) -> Result<PathBuf> {
    if !explicit.is_empty() {
        let p = PathBuf::from(explicit);
        std::fs::metadata(&p).with_context(|| format!("credentials file {}", p.display()))?;
        return Ok(p);
    }
    let mut candidates = Vec::new();
    if let Ok(p) = std::env::var("CLAUDE_CREDENTIALS_PATH") {
        candidates.push(PathBuf::from(p));
    }
    if let Ok(d) = std::env::var("CLAUDE_CONFIG_DIR") {
        candidates.push(Path::new(&d).join(".credentials.json"));
    }
    let home_default = dirs::home_dir()
        .ok_or_else(|| anyhow!("cannot determine home directory"))?
        .join(".claude")
        .join(".credentials.json");
    candidates.push(home_default.clone());
    if let Some(p) = candidates.into_iter().find(|p| p.is_file()) {
        return Ok(p);
    }
    if cfg!(target_os = "macos") {
        bail!(
            "credentials not found at {0}; on macOS Claude Code stores them in the keychain — export with: security find-generic-password -s 'Claude Code-credentials' -w > {0}",
            home_default.display()
        );
    }
    bail!(
        "credentials not found at {} — log in with `claude /login` first",
        home_default.display()
    )
}

/// Atomically rewrites a credentials file (0600) in the `claudeAiOauth` shape.
pub fn write_credentials_file(path: &Path, c: &Credentials) -> Result<()> {
    let body = serde_json::to_vec_pretty(&CredsFile {
        claude_ai_oauth: c.clone(),
    })?;
    write_private_atomic(path, &body)
        .with_context(|| format!("write credentials {}", path.display()))
}

/// Writes `body` to a temp file (0600) next to `path`, then renames it
/// over `path`.
pub(crate) fn write_private_atomic(path: &Path, body: &[u8]) -> Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = dir.join(format!(".{name}-{}.tmp", std::process::id()));
    let result = (|| -> Result<()> {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp)?;
        f.write_all(body)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[derive(Serialize)]
struct RefreshRequest<'a> {
    grant_type: &'a str,
    refresh_token: &'a str,
    client_id: &'a str,
}

#[derive(Deserialize)]
struct RefreshResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: String,
    #[serde(default)]
    expires_in: i64,
}

/// OAuth token-endpoint client.
#[derive(Clone)]
pub struct OAuthClient {
    http: reqwest::Client,
    token_url: String,
    client_id: String,
}

impl OAuthClient {
    pub fn new(token_url: impl Into<String>, client_id: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("build reqwest client"),
            token_url: token_url.into(),
            client_id: client_id.into(),
        }
    }

    /// The same endpoint reached through an HTTP / SOCKS5 proxy.
    pub fn with_proxy(&self, proxy_url: &str) -> Result<Self> {
        Ok(Self {
            http: proxied_client(proxy_url, Some(Duration::from_secs(30)))?,
            ..self.clone()
        })
    }

    /// Exchanges `cur.refresh_token` for a new token set. Scopes, and the
    /// refresh token when the endpoint doesn't rotate it, carry over.
    pub async fn refresh(&self, cur: &Credentials) -> Result<Credentials> {
        if cur.refresh_token.is_empty() {
            bail!("no refresh token available");
        }
        let resp = self
            .http
            .post(&self.token_url)
            .header("Accept", "application/json")
            .json(&RefreshRequest {
                grant_type: "refresh_token",
                refresh_token: &cur.refresh_token,
                client_id: &self.client_id,
            })
            .send()
            .await?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!(
                "token endpoint returned {}: {}",
                status.as_u16(),
                truncate(&body, 200)
            );
        }
        let r: RefreshResponse = serde_json::from_str(&body).context("decode refresh response")?;
        if r.access_token.is_empty() {
            bail!("refresh response missing access_token");
        }
        Ok(Credentials {
            access_token: r.access_token,
            refresh_token: if r.refresh_token.is_empty() {
                cur.refresh_token.clone()
            } else {
                r.refresh_token
            },
            expires_at: crate::now_millis() + r.expires_in * 1000,
            scopes: cur.scopes.clone(),
        })
    }
}

/// A reqwest client that sends everything through `proxy_url` (never
/// directly). `timeout` is the overall request timeout, if any.
pub fn proxied_client(proxy_url: &str, timeout: Option<Duration>) -> Result<reqwest::Client> {
    let mut b = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(proxy_url).with_context(|| format!("proxy {proxy_url:?}"))?)
        .connect_timeout(Duration::from_secs(30));
    if let Some(t) = timeout {
        b = b.timeout(t);
    }
    Ok(b.build()?)
}

/// Whether a refresh error means the refresh token itself was rejected.
pub fn is_invalid_grant(err: &anyhow::Error) -> bool {
    let s = err.to_string();
    s.contains("invalid_grant")
        || s.contains("Refresh token not found")
        || s.contains("refresh_token_invalid")
}

pub(crate) fn truncate(s: &str, n: usize) -> String {
    match s.char_indices().nth(n) {
        Some((i, _)) => format!("{}...", &s[..i]),
        None => s.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_shapes() {
        let cases: &[(&str, &str)] = &[
            (
                r#"{"claudeAiOauth":{"accessToken":"a","refreshToken":"r","expiresAt":1730000000000}}"#,
                "r",
            ),
            (
                r#"{"accessToken":"a","refreshToken":"r","expiresAt":1730000000000}"#,
                "r",
            ),
            (
                r#"{"access_token":"a","refresh_token":"r","expires_at":1730000000}"#,
                "r",
            ),
        ];
        for (body, refresh) in cases {
            let c = parse_credentials(body.as_bytes()).unwrap();
            assert_eq!(c.access_token, "a");
            assert_eq!(c.refresh_token, *refresh);
            assert_eq!(c.expires_at, 1_730_000_000_000, "{body}");
        }
        let c = parse_credentials(br#"{"access_token":"a","expires_in":60}"#).unwrap();
        assert!(c.expires_at > crate::now_millis());
        assert!(parse_credentials(br#"{"foo":1}"#).is_err());
    }

    #[test]
    fn write_then_read_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".credentials.json");
        let c = Credentials {
            access_token: "a".into(),
            refresh_token: "r".into(),
            expires_at: 5,
            scopes: vec!["s".into()],
        };
        write_credentials_file(&path, &c).unwrap();
        assert_eq!(read_credentials_file(&path).unwrap(), c);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("claudeAiOauth"));
    }
}
