//! The shared proxy list: named upstream proxies that accounts can be
//! assigned to (an assignment writes the URL into `accounts.proxy_url`,
//! which is what requests actually use).

use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use rusqlite::params;
use serde::Serialize;

use crate::store::Store;

#[derive(Debug, Clone, Serialize)]
pub struct ProxyEntry {
    pub id: i64,
    pub name: String,
    pub url: String,
    pub created_at: i64,
    /// Accounts whose `proxy_url` is this URL.
    pub accounts: Vec<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProxyTest {
    pub ok: bool,
    pub latency_ms: i64,
    /// HTTP status from the upstream base URL through the proxy, if any.
    pub status: Option<u16>,
    pub error: String,
}

#[derive(Clone)]
pub struct ProxyList {
    store: Store,
}

/// `http`, `https`, `socks5` and `socks5h` proxies, as reqwest supports.
pub fn validate_url(url: &str) -> Result<()> {
    let ok = ["http://", "https://", "socks5://", "socks5h://"]
        .iter()
        .any(|p| url.starts_with(p));
    if !ok {
        bail!("proxy URL must start with http://, https://, socks5:// or socks5h://");
    }
    reqwest::Proxy::all(url)?;
    Ok(())
}

impl ProxyList {
    pub fn new(store: Store) -> Self {
        Self { store }
    }

    pub fn list(&self) -> Result<Vec<ProxyEntry>> {
        let conn = self.store.conn();
        let mut stmt = conn.prepare("SELECT id, name, url, created_at FROM proxies ORDER BY id")?;
        let mut out: Vec<ProxyEntry> = stmt
            .query_map([], |r| {
                Ok(ProxyEntry {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    url: r.get(2)?,
                    created_at: r.get(3)?,
                    accounts: vec![],
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        let mut using = conn.prepare("SELECT id FROM accounts WHERE proxy_url = ?1 ORDER BY id")?;
        for p in &mut out {
            p.accounts = using
                .query_map([&p.url], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
        }
        Ok(out)
    }

    pub fn get(&self, id: i64) -> Result<ProxyEntry> {
        match self.list()?.into_iter().find(|p| p.id == id) {
            Some(p) => Ok(p),
            None => bail!("proxy {id} not found"),
        }
    }

    pub fn add(&self, name: &str, url: &str) -> Result<i64> {
        let url = url.trim();
        validate_url(url)?;
        let name = if name.trim().is_empty() {
            url
        } else {
            name.trim()
        };
        let conn = self.store.conn();
        conn.execute(
            "INSERT INTO proxies(name, url, created_at) VALUES(?1, ?2, ?3)",
            params![name, url, crate::now_unix()],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Removes the entry; accounts keep their proxy URL until reassigned.
    pub fn delete(&self, id: i64) -> Result<()> {
        if self
            .store
            .conn()
            .execute("DELETE FROM proxies WHERE id = ?1", [id])?
            == 0
        {
            bail!("proxy {id} not found");
        }
        Ok(())
    }

    /// Reaches `target` (the upstream base URL) through the proxy. Any HTTP
    /// answer counts as working: the point is the network path.
    pub async fn test(url: &str, target: &str) -> ProxyTest {
        let start = Instant::now();
        let client = match crate::oauth::proxied_client(url, Some(Duration::from_secs(15))) {
            Ok(c) => c,
            Err(e) => {
                return ProxyTest {
                    ok: false,
                    latency_ms: 0,
                    status: None,
                    error: format!("{e:#}"),
                }
            }
        };
        let r = client.get(target).send().await;
        let latency_ms = start.elapsed().as_millis() as i64;
        match r {
            Ok(resp) => ProxyTest {
                ok: true,
                latency_ms,
                status: Some(resp.status().as_u16()),
                error: String::new(),
            },
            Err(e) => ProxyTest {
                ok: false,
                latency_ms,
                status: None,
                error: format!("{e}"),
            },
        }
    }
}
