//! Pointing this machine's Claude Code at CC Manager: sets
//! `env.ANTHROPIC_BASE_URL` / `env.ANTHROPIC_AUTH_TOKEN` in
//! `~/.claude/settings.json`, keeping a backup of the original so it can
//! be restored. Everything else in the file is left as it was.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::oauth::write_private_atomic;

/// Env keys this module owns. `ANTHROPIC_API_KEY` is removed on sync:
/// with both set, Claude Code asks which one to use.
const BASE_URL: &str = "ANTHROPIC_BASE_URL";
const AUTH_TOKEN: &str = "ANTHROPIC_AUTH_TOKEN";
const API_KEY: &str = "ANTHROPIC_API_KEY";

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub path: String,
    pub exists: bool,
    /// `env.ANTHROPIC_BASE_URL`, if set.
    pub base_url: Option<String>,
    /// Whether a token or key is set (never the value).
    pub has_token: bool,
    /// A backup from the first sync exists, so restore is possible.
    pub backup: bool,
}

/// What the file looked like before the first sync.
#[derive(Serialize, Deserialize)]
struct Backup {
    existed: bool,
    content: String,
}

pub struct ClaudeCodeSettings {
    path: PathBuf,
}

impl ClaudeCodeSettings {
    /// `explicit` when set, else `~/.claude/settings.json`.
    pub fn new(explicit: &str) -> Result<Self> {
        let path = if explicit.is_empty() {
            dirs::home_dir()
                .context("no home directory")?
                .join(".claude")
                .join("settings.json")
        } else {
            PathBuf::from(explicit)
        };
        Ok(Self { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn backup_path(&self) -> PathBuf {
        let mut p = self.path.clone().into_os_string();
        p.push(".cc-manager-backup");
        PathBuf::from(p)
    }

    fn read(&self) -> Result<Option<(String, Value)>> {
        match std::fs::read_to_string(&self.path) {
            Ok(s) => {
                let v = if s.trim().is_empty() {
                    json!({})
                } else {
                    serde_json::from_str(&s)
                        .with_context(|| format!("parse {}", self.path.display()))?
                };
                Ok(Some((s, v)))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("read {}", self.path.display())),
        }
    }

    pub fn status(&self) -> Result<Status> {
        let current = self.read()?;
        let env = current.as_ref().and_then(|(_, v)| v.get("env"));
        let get = |k: &str| env.and_then(|e| e.get(k)).and_then(Value::as_str);
        Ok(Status {
            path: self.path.display().to_string(),
            exists: current.is_some(),
            base_url: get(BASE_URL).map(String::from),
            has_token: get(AUTH_TOKEN).is_some() || get(API_KEY).is_some(),
            backup: self.backup_path().exists(),
        })
    }

    /// Writes the base URL and token, backing up the original file the
    /// first time (later syncs keep that first backup).
    pub fn apply(&self, base_url: &str, token: &str) -> Result<Status> {
        if !(base_url.starts_with("http://") || base_url.starts_with("https://")) {
            bail!("base URL must start with http:// or https://");
        }
        let current = self.read()?;
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        if !self.backup_path().exists() {
            let backup = Backup {
                existed: current.is_some(),
                content: current.as_ref().map(|(s, _)| s.clone()).unwrap_or_default(),
            };
            write_private_atomic(&self.backup_path(), &serde_json::to_vec(&backup)?)
                .context("write backup")?;
        }
        let mut doc = current.map(|(_, v)| v).unwrap_or_else(|| json!({}));
        let Some(obj) = doc.as_object_mut() else {
            bail!("{} is not a JSON object", self.path.display());
        };
        let env = obj
            .entry("env")
            .or_insert_with(|| Value::Object(Map::new()));
        let Some(env) = env.as_object_mut() else {
            bail!("\"env\" in {} is not an object", self.path.display());
        };
        env.insert(BASE_URL.into(), json!(base_url.trim_end_matches('/')));
        env.insert(AUTH_TOKEN.into(), json!(token));
        env.remove(API_KEY);
        let mut body = serde_json::to_vec_pretty(&doc)?;
        body.push(b'\n');
        write_private_atomic(&self.path, &body)
            .with_context(|| format!("write {}", self.path.display()))?;
        self.status()
    }

    /// Puts the original file back (or removes it if there was none) and
    /// drops the backup.
    pub fn restore(&self) -> Result<Status> {
        let raw = std::fs::read(self.backup_path()).context("no backup to restore")?;
        let backup: Backup = serde_json::from_slice(&raw).context("parse backup")?;
        if backup.existed {
            write_private_atomic(&self.path, backup.content.as_bytes())?;
        } else if self.path.exists() {
            std::fs::remove_file(&self.path)?;
        }
        std::fs::remove_file(self.backup_path())?;
        self.status()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_keeps_other_settings_and_restores() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".claude/settings.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = r#"{"model":"opus","env":{"FOO":"1","ANTHROPIC_API_KEY":"old"}}"#;
        std::fs::write(&path, original).unwrap();
        let s = ClaudeCodeSettings::new(path.to_str().unwrap()).unwrap();

        let st = s.apply("http://10.0.0.2:8787/", "sk-cp-new").unwrap();
        assert_eq!(st.base_url.as_deref(), Some("http://10.0.0.2:8787"));
        assert!(st.has_token && st.backup);
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["model"], "opus");
        assert_eq!(v["env"]["FOO"], "1");
        assert_eq!(v["env"]["ANTHROPIC_AUTH_TOKEN"], "sk-cp-new");
        assert!(v["env"].get("ANTHROPIC_API_KEY").is_none());

        // A second sync keeps the first backup.
        s.apply("http://10.0.0.2:8787", "sk-cp-newer").unwrap();
        let st = s.restore().unwrap();
        assert!(!st.backup);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        assert!(s.restore().is_err());
    }

    #[test]
    fn restore_removes_a_file_that_did_not_exist() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let s = ClaudeCodeSettings::new(path.to_str().unwrap()).unwrap();
        assert!(!s.status().unwrap().exists);
        s.apply("https://cc.lan", "tok").unwrap();
        assert!(path.exists());
        assert!(!s.restore().unwrap().exists);
    }
}
