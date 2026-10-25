//! "Check for updates": the latest GitHub release against this build.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
pub struct UpdateInfo {
    pub current: String,
    pub latest: String,
    pub newer: bool,
    pub url: String,
    pub notes: String,
    pub published_at: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Release {
    tag_name: String,
    html_url: String,
    body: String,
    published_at: String,
}

/// `v1.2.3` / `1.2.3-beta` → (1, 2, 3); missing parts are 0.
fn parse(v: &str) -> (u64, u64, u64) {
    let core = v.trim().trim_start_matches('v');
    let core = core.split(['-', '+']).next().unwrap_or_default();
    let mut it = core.split('.').map(|p| p.parse::<u64>().unwrap_or(0));
    (
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
    )
}

pub fn is_newer(latest: &str, current: &str) -> bool {
    parse(latest) > parse(current)
}

/// Fetches `url` (a GitHub "latest release" API URL).
pub async fn check(url: &str) -> Result<UpdateInfo> {
    let r: Release = reqwest::Client::new()
        .get(url)
        .header("user-agent", format!("cc-manager/{}", crate::VERSION))
        .header("accept", "application/vnd.github+json")
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .context("fetch latest release")?
        .error_for_status()
        .context("fetch latest release")?
        .json()
        .await
        .context("parse latest release")?;
    Ok(UpdateInfo {
        current: crate::VERSION.to_string(),
        newer: is_newer(&r.tag_name, crate::VERSION),
        latest: r.tag_name,
        url: r.html_url,
        notes: r.body.chars().take(4000).collect(),
        published_at: r.published_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_compare() {
        assert!(is_newer("v0.2.0", "0.1.9"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("v0.1.0-beta", "0.1.0"));
        assert!(!is_newer("garbage", "0.1.0"));
    }
}
