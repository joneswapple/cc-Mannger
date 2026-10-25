//! CC Manager core: storage, encryption, Claude OAuth accounts and the HTTP
//! server shared by the desktop app (`src-tauri`) and `ccm-server`.

pub mod accounts;
pub mod analytics;
pub mod auth;
pub mod capture;
pub mod claude_code;
pub mod config;
pub mod events;
pub mod http;
pub mod login;
mod metrics;
pub mod monitor;
pub mod oauth;
mod onboard;
pub mod openai;
pub mod policy;
pub mod pool;
pub mod provider;
pub mod quota;
pub mod rewrite;
pub mod routing;
pub mod runtime;
pub mod secret;
pub mod stats;
pub mod store;
pub mod upstream;

use std::sync::Arc;

use anyhow::{Context, Result};

use crate::accounts::AccountStore;
use crate::analytics::Analytics;
use crate::auth::{Admin, Invites, Keys};
use crate::config::Config;
use crate::login::LoginManager;
use crate::monitor::Monitor;
use crate::oauth::OAuthClient;
use crate::policy::Engine;
use crate::pool::AccountPool;
use crate::routing::ModelRouter;
use crate::runtime::{Runtime, RuntimeConfig};
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
    pub analytics: Analytics,
    pub upstream: Upstream,
    pub catalog: Catalog,
    pub login: LoginManager,
    pub routing: ModelRouter,
    pub runtime: Runtime,
    pub monitor: Monitor,
    /// The address the proxy listener is bound to, while it runs.
    listening: Arc<std::sync::RwLock<Option<String>>>,
    /// Start / stop hooks for the listener, when the host can provide them
    /// (the desktop app); `ccm-server` runs exactly one listener.
    service: Arc<std::sync::OnceLock<Arc<dyn ServiceControl>>>,
    /// Guards [`Core::start_background`].
    background: Arc<std::sync::Once>,
}

/// Lets the admin API start and stop the proxy listener.
pub trait ServiceControl: Send + Sync {
    fn start(&self) -> Result<()>;
    fn stop(&self) -> Result<()>;
    /// Stop, then start on the current `runtime.listen`.
    fn restart(&self) -> Result<()>;
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
        let runtime = Runtime::load(store.clone(), &config)?;
        let rt = runtime.get();
        let pool = AccountPool::new(accounts.clone(), oauth, rt.pool.clone());
        let upstream = Upstream::new(&config.upstream, pool.clone());
        Ok((
            Core {
                keys: Keys::new(store.clone()),
                invites: Invites::new(store.clone()),
                admin: Admin::new(store.clone()),
                policy: Engine::new(store.clone()),
                recorder: Recorder::new(store.clone()),
                analytics: Analytics::new(store.clone()),
                catalog: Catalog::new(upstream.clone()),
                login: LoginManager::new(&config.upstream),
                routing: ModelRouter::new(store.clone(), rt.routing.clone()),
                monitor: Monitor::new(rt.monitor.clone()),
                runtime,
                listening: Default::default(),
                service: Default::default(),
                background: Arc::new(std::sync::Once::new()),
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

impl Core {
    /// Saves a settings patch (see [`Runtime::update`]) and applies it to
    /// the components that read it.
    pub fn update_runtime(&self, patch: &serde_json::Value) -> Result<RuntimeConfig> {
        let c = self.runtime.update(patch)?;
        self.pool.set_config(c.pool.clone());
        self.routing.set_config(c.routing.clone());
        self.monitor.set_config(c.monitor.clone());
        Ok(c)
    }

    /// Installs the listener controls; only the first call takes effect.
    pub fn set_service_control(&self, control: Arc<dyn ServiceControl>) {
        let _ = self.service.set(control);
    }

    pub fn service_control(&self) -> Option<Arc<dyn ServiceControl>> {
        self.service.get().cloned()
    }

    /// Where the listener is bound, or `None` when it isn't running.
    pub fn listening(&self) -> Option<String> {
        self.listening
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub(crate) fn set_listening(&self, addr: Option<String>) {
        *self.listening.write().unwrap_or_else(|e| e.into_inner()) = addr;
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
