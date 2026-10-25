//! Settings that can change while running: the part of [`Config`] the admin
//! UI edits. Saved overrides are a JSON merge patch (RFC 7386) kept in
//! `global_settings` under `runtime_overrides` and applied over the values
//! from defaults → `config.json` → `CCM_*` env, so a UI save wins over the
//! file. [`crate::Core::update_runtime`] pushes a change to the components.

use std::net::SocketAddr;
use std::sync::{Arc, RwLock};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::config::{
    Config, DesktopConfig, MonitorConfig, PoolConfig, RoutingConfig, SecurityConfig,
};
use crate::store::Store;

const SETTING_OVERRIDES: &str = "runtime_overrides";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RuntimeConfig {
    /// Listen address. A change applies when the listener restarts (the
    /// desktop app restarts it; `ccm-server` needs a restart).
    pub listen: String,
    pub pool: PoolConfig,
    pub routing: RoutingConfig,
    pub monitor: MonitorConfig,
    pub security: SecurityConfig,
    pub desktop: DesktopConfig,
    /// Seconds between polls of the optional usage endpoint.
    pub usage_poll_secs: u64,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self::from_config(&Config::default())
    }
}

impl RuntimeConfig {
    pub fn from_config(c: &Config) -> Self {
        Self {
            listen: c.listen.clone(),
            pool: c.pool.clone(),
            routing: c.routing.clone(),
            monitor: c.monitor.clone(),
            security: c.security.clone(),
            desktop: c.desktop.clone(),
            usage_poll_secs: c.upstream.usage_poll_secs,
        }
    }

    pub fn validate(&self) -> Result<()> {
        self.listen
            .parse::<SocketAddr>()
            .with_context(|| format!("listen {:?} is not host:port", self.listen))?;
        let p = &self.pool;
        if !(p.quota_soft_limit > 0.0 && p.quota_soft_limit <= 1.0) {
            bail!("pool.quota_soft_limit must be in (0, 1]");
        }
        if p.max_retries > 10 {
            bail!("pool.max_retries must be at most 10");
        }
        if !(1..=10_000).contains(&self.monitor.capacity) {
            bail!("monitor.capacity must be between 1 and 10000");
        }
        if self.monitor.max_body_bytes > 4 << 20 {
            bail!("monitor.max_body_bytes must be at most 4 MiB");
        }
        if self.usage_poll_secs < 60 {
            bail!("usage_poll_secs must be at least 60");
        }
        self.security.validate()
    }
}

/// The effective runtime settings. Cheap to clone.
#[derive(Clone)]
pub struct Runtime {
    store: Store,
    base: Arc<Value>,
    current: Arc<RwLock<RuntimeConfig>>,
}

impl Runtime {
    /// Base values from `cfg`, with saved overrides on top. Overrides that
    /// no longer validate are ignored (and logged) rather than blocking
    /// startup.
    pub fn load(store: Store, cfg: &Config) -> Result<Self> {
        let base = serde_json::to_value(RuntimeConfig::from_config(cfg))?;
        let rt = Self {
            store,
            current: Arc::new(RwLock::new(serde_json::from_value(base.clone())?)),
            base: Arc::new(base),
        };
        let overrides = rt.overrides()?;
        match rt.effective(&overrides) {
            Ok(c) => *rt.current.write().unwrap_or_else(|e| e.into_inner()) = c,
            Err(e) => tracing::warn!("saved runtime settings ignored: {e:#}"),
        }
        Ok(rt)
    }

    pub fn get(&self) -> RuntimeConfig {
        self.current
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// The saved merge patch (`{}` when nothing was changed from the UI).
    pub fn overrides(&self) -> Result<Value> {
        Ok(match self.store.get_setting(SETTING_OVERRIDES)? {
            Some(v) if !v.is_empty() => serde_json::from_str(&v)?,
            _ => Value::Object(Map::new()),
        })
    }

    /// The values from config.json / env, before overrides.
    pub fn base(&self) -> &Value {
        &self.base
    }

    /// Merges `patch` into the saved overrides (`null` resets a field to
    /// its config value), validates, saves and returns the new settings.
    pub fn update(&self, patch: &Value) -> Result<RuntimeConfig> {
        if !patch.is_object() {
            bail!("settings patch must be a JSON object");
        }
        check_known(&self.base, patch, "")?;
        let mut overrides = self.overrides()?;
        merge_patch(&mut overrides, patch);
        let next = self.effective(&overrides)?;
        self.store
            .set_setting(SETTING_OVERRIDES, &serde_json::to_string(&overrides)?)?;
        *self.current.write().unwrap_or_else(|e| e.into_inner()) = next.clone();
        Ok(next)
    }

    fn effective(&self, overrides: &Value) -> Result<RuntimeConfig> {
        let mut v = (*self.base).clone();
        merge_patch(&mut v, overrides);
        let c: RuntimeConfig = serde_json::from_value(v).context("invalid settings")?;
        c.validate()?;
        Ok(c)
    }
}

/// RFC 7386: objects merge recursively, `null` removes, anything else
/// replaces.
fn merge_patch(target: &mut Value, patch: &Value) {
    let Value::Object(p) = patch else {
        *target = patch.clone();
        return;
    };
    if !target.is_object() {
        *target = Value::Object(Map::new());
    }
    let t = target.as_object_mut().expect("object");
    for (k, v) in p {
        if v.is_null() {
            t.remove(k);
        } else {
            merge_patch(t.entry(k.clone()).or_insert(Value::Null), v);
        }
    }
}

/// Every key in `patch` must name an existing setting, so typos fail
/// loudly instead of being saved and ignored. Maps (whose keys are data,
/// not field names) accept any key.
fn check_known(base: &Value, patch: &Value, path: &str) -> Result<()> {
    let (Value::Object(b), Value::Object(p)) = (base, patch) else {
        return Ok(());
    };
    for (k, v) in p {
        let here = if path.is_empty() {
            k.clone()
        } else {
            format!("{path}.{k}")
        };
        match b.get(k) {
            Some(bv) => check_known(bv, v, &here)?,
            None => bail!("unknown setting {here:?}"),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rt() -> Runtime {
        Runtime::load(Store::open_in_memory().unwrap(), &Config::default()).unwrap()
    }

    #[test]
    fn patch_overrides_and_null_resets() {
        let r = rt();
        assert_eq!(r.get().pool.max_retries, 2);
        let c = r
            .update(&json!({"pool": {"max_retries": 4, "strategy": "least_used"}}))
            .unwrap();
        assert_eq!(c.pool.max_retries, 4);
        assert_eq!(c.pool.sticky_ttl_secs, 3600, "untouched fields keep base");
        // Survives a reload from the same store.
        let again = Runtime::load(r.store.clone(), &Config::default()).unwrap();
        assert_eq!(
            again.get().pool.strategy,
            crate::config::Strategy::LeastUsed
        );
        let c = r.update(&json!({"pool": {"max_retries": null}})).unwrap();
        assert_eq!(c.pool.max_retries, 2);
        assert_eq!(
            r.overrides().unwrap(),
            json!({"pool": {"strategy": "least_used"}})
        );
    }

    #[test]
    fn rejects_unknown_and_invalid_without_saving() {
        let r = rt();
        assert!(r.update(&json!({"pool": {"max_retry": 1}})).is_err());
        assert!(r.update(&json!({"listen": "nope"})).is_err());
        assert!(r
            .update(&json!({"pool": {"quota_soft_limit": 1.5}}))
            .is_err());
        assert!(r.update(&json!({"pool": {"strategy": "random"}})).is_err());
        assert_eq!(r.overrides().unwrap(), json!({}));
        assert_eq!(r.get(), RuntimeConfig::default());
    }
}
