//! Model routing: rewrite the requested model before forwarding.
//!
//! Routes are an ordered list of `{pattern, target}` kept in
//! `global_settings`; the first match wins. A pattern is an exact model name
//! (case-insensitive) or, prefixed with `re:`, a regular expression. An
//! optional "background" rule sends small tool-less requests (titles,
//! summaries) to a cheaper model.

use std::sync::{Arc, RwLock};

use anyhow::{bail, Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::RoutingConfig;
use crate::store::Store;

const SETTING_ROUTES: &str = "model_routes";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelRoute {
    pub pattern: String,
    pub target: String,
}

impl ModelRoute {
    fn matches(&self, model: &str) -> bool {
        match self.pattern.strip_prefix("re:") {
            Some(re) => Regex::new(re).is_ok_and(|r| r.is_match(model)),
            None => self.pattern.eq_ignore_ascii_case(model),
        }
    }
}

#[derive(Clone)]
pub struct ModelRouter {
    store: Store,
    cfg: Arc<RwLock<RoutingConfig>>,
}

impl ModelRouter {
    pub fn new(store: Store, cfg: RoutingConfig) -> Self {
        Self {
            store,
            cfg: Arc::new(RwLock::new(cfg)),
        }
    }

    pub fn config(&self) -> RoutingConfig {
        self.cfg.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn set_config(&self, cfg: RoutingConfig) {
        *self.cfg.write().unwrap_or_else(|e| e.into_inner()) = cfg;
    }

    pub fn routes(&self) -> Result<Vec<ModelRoute>> {
        match self.store.get_setting(SETTING_ROUTES)? {
            Some(v) if !v.is_empty() => Ok(serde_json::from_str(&v)?),
            _ => Ok(vec![]),
        }
    }

    /// Replaces the route list after validating every entry.
    pub fn set_routes(&self, routes: &[ModelRoute]) -> Result<()> {
        for r in routes {
            if r.pattern.trim().is_empty() || r.target.trim().is_empty() {
                bail!("each route needs a pattern and a target");
            }
            if let Some(re) = r.pattern.strip_prefix("re:") {
                Regex::new(re)
                    .with_context(|| format!("invalid regex in route {:?}", r.pattern))?;
            }
        }
        self.store
            .set_setting(SETTING_ROUTES, &serde_json::to_string(routes)?)
    }

    /// The model to actually use for a request body asking for `model`.
    pub fn resolve(&self, model: &str, body: &Value) -> Result<String> {
        if let Some(r) = self.routes()?.iter().find(|r| r.matches(model)) {
            return Ok(r.target.clone());
        }
        if self.is_background(body) {
            return Ok(self.config().background_model);
        }
        Ok(model.to_string())
    }

    /// A small request without tools (title generation, quick summaries),
    /// when the background rule is enabled.
    fn is_background(&self, body: &Value) -> bool {
        let cfg = self.config();
        if cfg.background_model.is_empty() {
            return false;
        }
        let small = body["max_tokens"]
            .as_i64()
            .is_some_and(|m| m <= cfg.background_max_tokens);
        let no_tools = body
            .get("tools")
            .and_then(Value::as_array)
            .is_none_or(|t| t.is_empty());
        small && no_tools
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn router(cfg: RoutingConfig) -> ModelRouter {
        ModelRouter::new(Store::open_in_memory().unwrap(), cfg)
    }

    fn route(p: &str, t: &str) -> ModelRoute {
        ModelRoute {
            pattern: p.into(),
            target: t.into(),
        }
    }

    #[test]
    fn exact_and_regex_first_match_wins() {
        let r = router(RoutingConfig::default());
        r.set_routes(&[
            route("GPT-4o", "claude-sonnet-4-5"),
            route("re:^gpt-", "claude-haiku-4-5"),
            route("re:.*", "never"),
        ])
        .unwrap();
        let body = json!({});
        assert_eq!(r.resolve("gpt-4o", &body).unwrap(), "claude-sonnet-4-5");
        assert_eq!(
            r.resolve("gpt-3.5-turbo", &body).unwrap(),
            "claude-haiku-4-5"
        );
        assert_eq!(r.resolve("claude-opus-4-1", &body).unwrap(), "never");
        assert_eq!(r.routes().unwrap().len(), 3);
    }

    #[test]
    fn rejects_bad_routes() {
        let r = router(RoutingConfig::default());
        assert!(r.set_routes(&[route("re:(", "x")]).is_err());
        assert!(r.set_routes(&[route("", "x")]).is_err());
        assert!(r.set_routes(&[route("a", " ")]).is_err());
    }

    #[test]
    fn background_downgrade_only_when_enabled_small_and_toolless() {
        let off = router(RoutingConfig::default());
        assert_eq!(
            off.resolve("claude-opus", &json!({"max_tokens": 100}))
                .unwrap(),
            "claude-opus"
        );
        let on = router(RoutingConfig {
            background_model: "claude-haiku-4-5".into(),
            background_max_tokens: 512,
        });
        assert_eq!(
            on.resolve("claude-opus", &json!({"max_tokens": 100}))
                .unwrap(),
            "claude-haiku-4-5"
        );
        assert_eq!(
            on.resolve("claude-opus", &json!({"max_tokens": 4096}))
                .unwrap(),
            "claude-opus"
        );
        assert_eq!(
            on.resolve(
                "claude-opus",
                &json!({"max_tokens": 100, "tools": [{"name": "x"}]})
            )
            .unwrap(),
            "claude-opus"
        );
    }
}
