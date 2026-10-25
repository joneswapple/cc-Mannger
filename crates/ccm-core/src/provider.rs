//! Turning stored accounts into bearer tokens for upstream calls.

use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use tokio::sync::Mutex as AsyncMutex;

use crate::accounts::{Account, AccountStore, Credentials, STATUS_ACTIVE, STATUS_NEEDS_REAUTH};
use crate::oauth::{self, OAuthClient};

/// Refresh this long before the stamped expiry.
const REFRESH_SKEW_MS: i64 = 60_000;

/// Bearer tokens for one account. Refreshes on demand and writes the new
/// token set back (encrypted) to the database. With `sync_file` set, the
/// account's source credentials file stays authoritative: it is re-read on
/// first use and before every refresh, and rewritten after a refresh, so the
/// host's `claude` CLI and CC Manager can keep rotating one refresh token.
pub struct AccountProvider {
    store: AccountStore,
    oauth: OAuthClient,
    id: i64,
    label: String,
    pub(crate) sync_path: Option<PathBuf>,
    state: AsyncMutex<State>,
}

#[derive(Default)]
struct State {
    creds: Option<Credentials>,
    warned_no_refresh: bool,
}

impl AccountProvider {
    pub fn new(store: AccountStore, oauth: OAuthClient, a: &Account) -> Self {
        Self {
            store,
            oauth,
            id: a.id,
            label: a.label.clone(),
            sync_path: (a.sync_file && !a.source_path.is_empty())
                .then(|| a.source_path.clone().into()),
            state: AsyncMutex::new(State::default()),
        }
    }

    pub fn account_id(&self) -> i64 {
        self.id
    }

    pub fn source(&self) -> String {
        match &self.sync_path {
            Some(p) => format!(
                "account #{} \"{}\" (synced with {})",
                self.id,
                self.label,
                p.display()
            ),
            None => format!("account #{} \"{}\"", self.id, self.label),
        }
    }

    /// Returns a usable access token, refreshing it first when it is within
    /// a minute of expiry.
    pub async fn token(&self) -> Result<String> {
        let mut st = self.state.lock().await;
        if st.creds.is_none() {
            st.creds = Some(self.load()?);
        }
        if !near_expiry(st.creds.as_ref().unwrap()) {
            return Ok(st.creds.as_ref().unwrap().access_token.clone());
        }
        let cur = self.adopt_file(st.creds.take().unwrap())?;
        if !near_expiry(&cur) {
            let tok = cur.access_token.clone();
            st.creds = Some(cur);
            return Ok(tok);
        }
        if cur.refresh_token.is_empty() {
            // Pro/Max access tokens often outlive their stamped expiry; let
            // the upstream 401 speak rather than failing up front.
            if !st.warned_no_refresh {
                tracing::warn!(account = self.id, "no refresh_token and expiresAt is in the past — using the existing access token");
                st.warned_no_refresh = true;
            }
            let tok = cur.access_token.clone();
            st.creds = Some(cur);
            return Ok(tok);
        }
        let result = self.refresh(&cur).await;
        match result {
            Ok(next) => {
                let tok = next.access_token.clone();
                st.creds = Some(next);
                Ok(tok)
            }
            Err(e) => {
                st.creds = Some(cur);
                Err(e.context(format!("oauth refresh (account #{})", self.id)))
            }
        }
    }

    /// Called after the upstream rejected the access token (401): forces a
    /// refresh on next use, or marks the account `needs_reauth` when there
    /// is nothing to refresh with.
    pub async fn invalidate(&self) {
        let mut st = self.state.lock().await;
        let can_refresh = match st.creds.as_mut() {
            Some(c) => {
                c.expires_at = 1;
                !c.refresh_token.is_empty()
            }
            None => true,
        };
        if !can_refresh && self.sync_path.is_none() {
            let reason = "upstream rejected the access token (401) and there is no refresh token";
            if let Err(e) = self.store.set_status(self.id, STATUS_NEEDS_REAUTH, reason) {
                tracing::warn!(account = self.id, "mark needs_reauth: {e:#}");
            }
        }
    }

    /// Stored credentials, overridden by the synced file when it differs.
    fn load(&self) -> Result<Credentials> {
        let c = self.store.load_credentials(self.id)?;
        self.adopt_file(c)
    }

    fn adopt_file(&self, cur: Credentials) -> Result<Credentials> {
        let Some(path) = &self.sync_path else {
            return Ok(cur);
        };
        let file = match oauth::read_credentials_file(path) {
            Ok(f) => f,
            Err(e) => {
                // The DB copy still works; the file may be mid-rewrite or gone.
                tracing::warn!(account = self.id, "cannot use synced credentials: {e:#}");
                return Ok(cur);
            }
        };
        if file.access_token == cur.access_token && file.refresh_token == cur.refresh_token {
            return Ok(cur);
        }
        self.store.save_credentials(self.id, &file)?;
        // A new token set (e.g. the host re-ran `claude /login`) clears any
        // earlier needs_reauth verdict.
        if let Err(e) = self.store.set_status(self.id, STATUS_ACTIVE, "") {
            tracing::warn!(account = self.id, "mark active: {e:#}");
        }
        Ok(file)
    }

    async fn refresh(&self, cur: &Credentials) -> Result<Credentials> {
        let err = match self.attempt_refresh(cur).await {
            Ok(next) => return Ok(next),
            Err(e) if !oauth::is_invalid_grant(&e) => return Err(e),
            Err(e) => e,
        };
        // Someone else (the host's `claude`, another instance) may have
        // rotated the refresh token. Retry once if what's stored changed.
        let err = match self.load() {
            Ok(fresh) if fresh.refresh_token != cur.refresh_token => {
                match self.attempt_refresh(&fresh).await {
                    Ok(next) => return Ok(next),
                    Err(e) => e,
                }
            }
            _ => err,
        };
        let reason = format!(
            "refresh token rejected: {}",
            oauth::truncate(&format!("{err:#}"), 200)
        );
        if let Err(e) = self.store.set_status(self.id, STATUS_NEEDS_REAUTH, &reason) {
            tracing::warn!(account = self.id, "mark needs_reauth: {e:#}");
        }
        Err(err.context("the account's refresh token is no longer accepted by Anthropic; log in again for this account"))
    }

    async fn attempt_refresh(&self, cur: &Credentials) -> Result<Credentials> {
        let next = self.oauth.refresh(cur).await?;
        self.store.save_credentials(self.id, &next)?;
        if let Some(path) = &self.sync_path {
            if let Err(e) = oauth::write_credentials_file(path, &next) {
                tracing::warn!(account = self.id, "write synced credentials: {e:#}");
            }
        }
        if let Err(e) = self.store.set_status(self.id, STATUS_ACTIVE, "") {
            tracing::warn!(account = self.id, "mark active: {e:#}");
        }
        Ok(next)
    }
}

fn near_expiry(c: &Credentials) -> bool {
    c.expires_at > 0 && crate::now_millis() > c.expires_at - REFRESH_SKEW_MS
}

/// Seeds an empty accounts table from the host's Claude Code credentials
/// file. Returns the new account id, or `None` when accounts already exist.
pub fn import_cli_credentials(
    store: &AccountStore,
    explicit_path: &str,
    sync_file: bool,
) -> Result<Option<i64>> {
    if store.count()? > 0 {
        return Ok(None);
    }
    let path = oauth::resolve_credentials_path(explicit_path)?;
    let creds = oauth::read_credentials_file(&path)?;
    let path = path
        .to_str()
        .ok_or_else(|| anyhow!("non-UTF-8 credentials path"))?;
    let id = store
        .create(
            Account {
                label: "claude-cli".into(),
                enabled: true,
                source_path: path.into(),
                sync_file,
                ..Default::default()
            },
            &creds,
        )
        .context("import Claude Code credentials")?;
    Ok(Some(id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::test_store;
    use axum::{routing::post, Json, Router};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// A fake OAuth token endpoint accepting only `good_refresh`.
    async fn token_server(good_refresh: &'static str) -> (String, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let app = Router::new().route(
            "/token",
            post(move |Json(req): Json<serde_json::Value>| {
                let c = c.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    if req["refresh_token"] == good_refresh {
                        (axum::http::StatusCode::OK, r#"{"access_token":"new-access","refresh_token":"new-refresh","expires_in":3600}"#)
                    } else {
                        (axum::http::StatusCode::BAD_REQUEST, r#"{"error":"invalid_grant"}"#)
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}/token"), calls)
    }

    fn creds(access: &str, refresh: &str, expires_at: i64) -> Credentials {
        Credentials {
            access_token: access.into(),
            refresh_token: refresh.into(),
            expires_at,
            scopes: vec![],
        }
    }

    fn write_file(dir: &std::path::Path, c: &Credentials) -> String {
        let p = dir.join(".credentials.json");
        oauth::write_credentials_file(&p, c).unwrap();
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn import_seeds_once() {
        let s = test_store();
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(
            dir.path(),
            &creds("a", "r", crate::now_millis() + 3_600_000),
        );
        let id = import_cli_credentials(&s, &path, true).unwrap().unwrap();
        assert_eq!(import_cli_credentials(&s, &path, true).unwrap(), None);
        assert_eq!(s.load_credentials(id).unwrap().access_token, "a");
        assert!(s.get(id).unwrap().sync_file);
    }

    #[tokio::test]
    async fn refreshes_and_syncs_file() {
        let (url, calls) = token_server("file-refresh").await;
        let s = test_store();
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(
            dir.path(),
            &creds("old", "file-refresh", crate::now_millis() - 1000),
        );
        let id = import_cli_credentials(&s, &path, true).unwrap().unwrap();
        let p = AccountProvider::new(s.clone(), OAuthClient::new(url, "cli"), &s.get(id).unwrap());

        assert_eq!(p.token().await.unwrap(), "new-access");
        assert_eq!(p.token().await.unwrap(), "new-access");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "fresh token must be cached"
        );
        assert_eq!(s.load_credentials(id).unwrap().refresh_token, "new-refresh");
        assert_eq!(
            oauth::read_credentials_file(path.as_ref())
                .unwrap()
                .refresh_token,
            "new-refresh"
        );
    }

    #[tokio::test]
    async fn adopts_token_rotated_by_host_before_refreshing() {
        let (url, calls) = token_server("unused").await;
        let s = test_store();
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(
            dir.path(),
            &creds("old", "old-refresh", crate::now_millis() - 1000),
        );
        let id = import_cli_credentials(&s, &path, true).unwrap().unwrap();
        let p = AccountProvider::new(s.clone(), OAuthClient::new(url, "cli"), &s.get(id).unwrap());
        assert!(p.token().await.is_err());
        // The host's `claude` logs in again and rewrites the file.
        write_file(
            dir.path(),
            &creds(
                "host-access",
                "host-refresh",
                crate::now_millis() + 3_600_000,
            ),
        );
        assert_eq!(p.token().await.unwrap(), "host-access");
        assert_eq!(s.get(id).unwrap().status, STATUS_ACTIVE);
        assert!(calls.load(Ordering::SeqCst) >= 1);
    }

    #[tokio::test]
    async fn invalid_grant_marks_needs_reauth() {
        let (url, _) = token_server("never").await;
        let s = test_store();
        let id = s
            .create(
                Account {
                    label: "x".into(),
                    enabled: true,
                    ..Default::default()
                },
                &creds("a", "r", crate::now_millis() - 1000),
            )
            .unwrap();
        let p = AccountProvider::new(s.clone(), OAuthClient::new(url, "cli"), &s.get(id).unwrap());
        assert!(p.token().await.is_err());
        assert_eq!(s.get(id).unwrap().status, STATUS_NEEDS_REAUTH);
    }

    #[tokio::test]
    async fn expired_without_refresh_token_uses_existing() {
        let s = test_store();
        let id = s
            .create(
                Account {
                    label: "x".into(),
                    enabled: true,
                    ..Default::default()
                },
                &creds("stale", "", crate::now_millis() - 1000),
            )
            .unwrap();
        let p = AccountProvider::new(
            s.clone(),
            OAuthClient::new("http://unused", "cli"),
            &s.get(id).unwrap(),
        );
        assert_eq!(p.token().await.unwrap(), "stale");
    }
}
