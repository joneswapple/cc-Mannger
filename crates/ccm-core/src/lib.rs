//! CC Manager core: storage, encryption, Claude OAuth accounts and the HTTP
//! server shared by the desktop app (`src-tauri`) and `ccm-server`.

pub mod accounts;
pub mod auth;
pub mod capture;
pub mod config;
pub mod http;
pub mod oauth;
pub mod policy;
pub mod pool;
pub mod provider;
pub mod rewrite;
pub mod secret;
pub mod stats;
pub mod store;
pub mod upstream;

use std::sync::Arc;

use anyhow::{Context, Result};

use crate::accounts::AccountStore;
use crate::auth::{Admin, Invites, Keys};
use crate::config::Config;
use crate::oauth::OAuthClient;
use crate::policy::Engine;
use crate::pool::AccountPool;
use crate::secret::SecretBox;
use crate::stats::Recorder;
use crate::store::Store;
use crate::upstream::{Catalog, Upstream};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Every long-lived component, wired once at startup. Cheap to clone.
#[derive(Clone)]
pub struct Core {
    pub config: Arc<Config>,
    pub store: Store,
    pub accounts: AccountStore,
    pub pool: AccountPool,
    pub keys: Keys,
    pub invites: Invites,
    pub admin: Admin,
    pub policy: Engine,
    pub recorder: Recorder,
    pub upstream: Upstream,
    pub catalog: Catalog,
}

/// What [`Core::open`] did on the way up, for the caller to report.
#[derive(Debug, Default)]
pub struct Startup {
    /// Set when the host's Claude Code credentials were imported as a new
    /// account because none existed yet.
    pub imported_account: Option<i64>,
    /// Why the import was skipped, when there are still no accounts. The
    /// server still starts so accounts can be added later.
    pub import_error: Option<String>,
}

impl Core {
    pub fn open(config: Config) -> Result<(Self, Startup)> {
        let store = Store::open(&config.data_dir)?;
        let secrets =
            SecretBox::load(&config.secret_key, &config.data_dir).context("load secret key")?;
        let accounts = AccountStore::new(store.clone(), Arc::new(secrets));
        let mut startup = Startup::default();
        match provider::import_cli_credentials(
            &accounts,
            &config.upstream.credentials_path,
            config.upstream.sync_credentials_file,
        ) {
            Ok(id) => startup.imported_account = id,
            Err(e) => startup.import_error = Some(format!("{e:#}")),
        }
        let oauth = OAuthClient::new(
            &config.upstream.oauth_token_url,
            &config.upstream.oauth_client_id,
        );
        let pool = AccountPool::new(accounts.clone(), oauth, config.pool.clone());
        let upstream = Upstream::new(&config.upstream, pool.clone());
        Ok((
            Core {
                keys: Keys::new(store.clone()),
                invites: Invites::new(store.clone()),
                admin: Admin::new(store.clone()),
                policy: Engine::new(store.clone()),
                recorder: Recorder::new(store.clone()),
                catalog: Catalog::new(upstream.clone()),
                upstream,
                config: Arc::new(config),
                store,
                accounts,
                pool,
            },
            startup,
        ))
    }
}

pub(crate) fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub(crate) fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
