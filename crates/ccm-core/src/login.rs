//! Adding upstream accounts: the Claude OAuth PKCE login (authorize URL →
//! `code#state` pasted back, or a redirect to `/oauth/callback`), and the
//! profile lookup that fills in email / organization / plan.
//!
//! None of these endpoints are publicly documented; every URL, the client id
//! and the scopes come from `UpstreamConfig`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64URL, Engine};
use rand::RngCore;
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::accounts::Credentials;
use crate::config::UpstreamConfig;

/// How long a started login stays redeemable.
const PENDING_TTL: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Clone, Serialize)]
pub struct LoginStart {
    /// Open this in a browser signed in to the Claude account.
    pub url: String,
    pub state: String,
    pub redirect_uri: String,
}

/// What a started login is waiting for.
#[derive(Debug, Clone, Default)]
pub struct LoginIntent {
    pub label: String,
    pub account_group: String,
}

struct Pending {
    verifier: String,
    intent: LoginIntent,
    created: Instant,
}

/// Account details from the OAuth token response or profile endpoint.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Profile {
    pub email: String,
    pub org_uuid: String,
    pub plan: String,
}

#[derive(Clone)]
pub struct LoginManager {
    http: reqwest::Client,
    authorize_url: String,
    token_url: String,
    client_id: String,
    redirect_uri: String,
    scopes: String,
    profile_url: String,
    pending: Arc<Mutex<HashMap<String, Pending>>>,
}

fn random_b64(bytes: usize) -> String {
    let mut b = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut b);
    B64URL.encode(b)
}

/// PKCE S256 challenge for a verifier.
pub fn code_challenge(verifier: &str) -> String {
    B64URL.encode(Sha256::digest(verifier.as_bytes()))
}

impl LoginManager {
    pub fn new(cfg: &UpstreamConfig) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("build reqwest client"),
            authorize_url: cfg.oauth_authorize_url.clone(),
            token_url: cfg.oauth_token_url.clone(),
            client_id: cfg.oauth_client_id.clone(),
            redirect_uri: cfg.oauth_redirect_uri.clone(),
            scopes: cfg.oauth_scopes.clone(),
            profile_url: cfg.profile_url.clone(),
            pending: Default::default(),
        }
    }

    /// Begins a login: returns the authorize URL to open.
    pub fn start(&self, intent: LoginIntent) -> LoginStart {
        let verifier = random_b64(32);
        let state = random_b64(32);
        let mut url = reqwest::Url::parse(&self.authorize_url).expect("valid oauth_authorize_url");
        url.query_pairs_mut()
            .append_pair("code", "true")
            .append_pair("client_id", &self.client_id)
            .append_pair("response_type", "code")
            .append_pair("redirect_uri", &self.redirect_uri)
            .append_pair("scope", &self.scopes)
            .append_pair("code_challenge", &code_challenge(&verifier))
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", &state);
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        pending.retain(|_, p| p.created.elapsed() < PENDING_TTL);
        pending.insert(
            state.clone(),
            Pending {
                verifier,
                intent,
                created: Instant::now(),
            },
        );
        LoginStart {
            url: url.into(),
            state,
            redirect_uri: self.redirect_uri.clone(),
        }
    }

    /// Finishes a login. `code` is what the callback page shows — usually
    /// `code#state` — or a bare code with `state` passed separately.
    pub async fn complete(
        &self,
        code: &str,
        state: Option<&str>,
    ) -> Result<(Credentials, Profile, LoginIntent)> {
        let code = code.trim();
        let (code, state) = match (code.split_once('#'), state) {
            (Some((c, s)), _) => (c.to_string(), s.to_string()),
            (None, Some(s)) => (code.to_string(), s.to_string()),
            (None, None) => bail!("missing state — paste the whole `code#state` value"),
        };
        let pending = self
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&state)
            .filter(|p| p.created.elapsed() < PENDING_TTL)
            .ok_or_else(|| anyhow!("unknown or expired login — start the login again"))?;
        let resp = self
            .http
            .post(&self.token_url)
            .header("Accept", "application/json")
            .json(&json!({
                "grant_type": "authorization_code",
                "code": code,
                "state": state,
                "client_id": self.client_id,
                "redirect_uri": self.redirect_uri,
                "code_verifier": pending.verifier,
            }))
            .send()
            .await?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!(
                "token endpoint returned {}: {}",
                status.as_u16(),
                crate::oauth::truncate(&body, 200)
            );
        }
        let v: Value = serde_json::from_str(&body).context("decode token response")?;
        let creds = Credentials {
            access_token: str_at(&v, &["access_token"]),
            refresh_token: str_at(&v, &["refresh_token"]),
            expires_at: crate::now_millis() + v["expires_in"].as_i64().unwrap_or(3600) * 1000,
            scopes: str_at(&v, &["scope"])
                .split_whitespace()
                .map(String::from)
                .collect(),
        };
        if creds.access_token.is_empty() {
            bail!("token response missing access_token");
        }
        Ok((creds, profile_from(&v), pending.intent))
    }

    /// Looks up the account behind an access token. Best effort: callers
    /// keep going without a profile.
    pub async fn profile(&self, access_token: &str) -> Result<Profile> {
        if self.profile_url.is_empty() {
            bail!("profile_url not configured");
        }
        let resp = self
            .http
            .get(&self.profile_url)
            .bearer_auth(access_token)
            .header("anthropic-beta", "oauth-2025-04-20")
            .header("Accept", "application/json")
            .send()
            .await?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!(
                "profile endpoint returned {}: {}",
                status.as_u16(),
                crate::oauth::truncate(&body, 200)
            );
        }
        Ok(profile_from(&serde_json::from_str(&body)?))
    }
}

fn str_at(v: &Value, path: &[&str]) -> String {
    let mut cur = v;
    for p in path {
        cur = &cur[*p];
    }
    cur.as_str().unwrap_or_default().to_string()
}

/// Reads email / organization / plan from either the token response or the
/// profile endpoint, whose field names differ slightly.
pub fn profile_from(v: &Value) -> Profile {
    let first = |paths: &[&[&str]]| {
        paths
            .iter()
            .map(|p| str_at(v, p))
            .find(|s| !s.is_empty())
            .unwrap_or_default()
    };
    let mut plan = first(&[
        &["organization", "organization_type"],
        &["organization", "rate_limit_tier"],
    ]);
    if plan.is_empty() {
        if v["account"]["has_claude_max"].as_bool() == Some(true) {
            plan = "claude_max".into();
        } else if v["account"]["has_claude_pro"].as_bool() == Some(true) {
            plan = "claude_pro".into();
        }
    }
    Profile {
        email: first(&[&["account", "email_address"], &["account", "email"]]),
        org_uuid: first(&[&["organization", "uuid"]]),
        plan,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{routing::post, Json, Router};

    #[test]
    fn pkce_challenge_matches_rfc7636_example() {
        assert_eq!(
            code_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn profile_shapes() {
        let token_resp = json!({"account": {"uuid": "u", "email_address": "a@x.io"}, "organization": {"uuid": "o"}});
        assert_eq!(
            profile_from(&token_resp),
            Profile {
                email: "a@x.io".into(),
                org_uuid: "o".into(),
                plan: "".into()
            }
        );
        let profile = json!({"account": {"email": "b@x.io", "has_claude_max": true}, "organization": {"uuid": "o2"}});
        assert_eq!(profile_from(&profile).plan, "claude_max");
        let typed = json!({"organization": {"organization_type": "claude_pro"}});
        assert_eq!(profile_from(&typed).plan, "claude_pro");
    }

    #[tokio::test]
    async fn login_round_trip_against_fake_token_endpoint() {
        let app = Router::new().route(
            "/token",
            post(|Json(req): Json<Value>| async move {
                // The verifier must hash to the challenge sent in the URL;
                // here we just check it was sent with the right grant.
                assert_eq!(req["grant_type"], "authorization_code");
                assert_eq!(req["code"], "the-code");
                assert!(!req["code_verifier"].as_str().unwrap().is_empty());
                Json(json!({
                    "access_token": "acc", "refresh_token": "ref", "expires_in": 3600,
                    "scope": "user:inference user:profile",
                    "account": {"email_address": "me@x.io"}, "organization": {"uuid": "org-1"}
                }))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let cfg = UpstreamConfig {
            oauth_token_url: format!("http://{addr}/token"),
            ..Default::default()
        };
        let m = LoginManager::new(&cfg);
        let start = m.start(LoginIntent {
            label: "mine".into(),
            ..Default::default()
        });
        let url = reqwest::Url::parse(&start.url).unwrap();
        let q: HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["state"], start.state);

        let (creds, profile, intent) = m
            .complete(&format!("the-code#{}", start.state), None)
            .await
            .unwrap();
        assert_eq!(
            (creds.access_token.as_str(), creds.refresh_token.as_str()),
            ("acc", "ref")
        );
        assert_eq!(creds.scopes.len(), 2);
        assert_eq!(profile.email, "me@x.io");
        assert_eq!(intent.label, "mine");
        // Single use.
        assert!(m
            .complete(&format!("the-code#{}", start.state), None)
            .await
            .is_err());
        assert!(m.complete("no-state", None).await.is_err());
    }
}
