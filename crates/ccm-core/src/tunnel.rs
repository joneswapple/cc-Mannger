//! Public access through a Cloudflare Tunnel: installs `cloudflared`, runs
//! it against a dedicated loopback listener and reports the public URL.
//!
//! The tunnel never points at the main listener. Its own listener (see
//! `http::public_router`) serves only the terminal-facing routes unless
//! `tunnel.expose_admin` is on, and takes the client IP from
//! `CF-Connecting-IP` so IP rules and limits keep working. The token for
//! token mode is sealed with the account [`SecretBox`] and handed to
//! `cloudflared` through `TUNNEL_TOKEN`, never on its command line.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use axum::Router;
use base64::Engine;
use serde::Serialize;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::oneshot;

use crate::config::{TunnelConfig, TunnelMode};
use crate::secret::SecretBox;
use crate::store::Store;

const TOKEN_SETTING: &str = "tunnel_token";
const TOKEN_AAD: &[u8] = b"tunnel:token";
const LOG_LINES: usize = 300;

#[derive(Debug, Clone, Default, Serialize)]
pub struct TunnelStatus {
    pub installed: bool,
    /// The `cloudflared` that would be (or is) run.
    pub binary: Option<String>,
    pub version: Option<String>,
    /// The binary is the one CC Manager downloaded.
    pub managed: bool,
    pub installing: bool,
    pub running: bool,
    /// Public URL once `cloudflared` reports it.
    pub url: Option<String>,
    /// The loopback listener the tunnel forwards to.
    pub local: Option<String>,
    pub started_at: Option<i64>,
    /// Why the tunnel stopped or failed to start.
    pub error: Option<String>,
    pub has_token: bool,
}

#[derive(Default)]
struct State {
    child: Option<Child>,
    listener_stop: Option<oneshot::Sender<()>>,
    /// Bumped on every start / stop so stale monitors and readers bow out.
    generation: u64,
    url: Option<String>,
    local: Option<String>,
    started_at: Option<i64>,
    error: Option<String>,
    installing: bool,
    logs: VecDeque<String>,
    version: Option<(PathBuf, String)>,
}

#[derive(Clone)]
pub struct Tunnel {
    store: Store,
    secrets: Arc<SecretBox>,
    bin_dir: PathBuf,
    download_url: String,
    state: Arc<Mutex<State>>,
}

fn exe_name() -> &'static str {
    if cfg!(windows) {
        "cloudflared.exe"
    } else {
        "cloudflared"
    }
}

/// The official release asset for this platform.
pub fn default_download_url() -> Result<String> {
    let (os, arch, ext) = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => ("darwin", "arm64", ".tgz"),
        ("macos", "x86_64") => ("darwin", "amd64", ".tgz"),
        ("linux", "x86_64") => ("linux", "amd64", ""),
        ("linux", "aarch64") => ("linux", "arm64", ""),
        ("linux", "arm") => ("linux", "arm", ""),
        ("windows", "x86_64") => ("windows", "amd64", ".exe"),
        ("windows", "x86") => ("windows", "386", ".exe"),
        (os, arch) => bail!(
            "no cloudflared build for {os}-{arch}; install it yourself and set tunnel.binary_path"
        ),
    };
    Ok(format!(
        "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-{os}-{arch}{ext}"
    ))
}

/// The public URL in a `cloudflared` log line: the quick-tunnel
/// `*.trycloudflare.com` address, or a named tunnel's first ingress
/// hostname from its "Updated to new configuration" line.
pub fn extract_url(line: &str) -> Option<String> {
    if let Some(u) = line
        .split(|c: char| c.is_whitespace() || c == '|')
        .find(|s| s.starts_with("https://") && s.contains(".trycloudflare.com"))
    {
        return Some(u.trim_end_matches(['/', '"']).to_string());
    }
    if line.contains("Updated to new configuration") && line.contains("hostname") {
        let unescaped = line.replace("\\\"", "\"");
        let key = "\"hostname\":\"";
        let start = unescaped.find(key)? + key.len();
        let host = &unescaped[start..];
        let host = &host[..host.find('"')?];
        if !host.is_empty() && !host.contains('*') {
            return Some(format!("https://{host}"));
        }
    }
    None
}

fn on_path(name: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

impl Tunnel {
    pub fn new(store: Store, secrets: Arc<SecretBox>, data_dir: &Path, download_url: &str) -> Self {
        Self {
            store,
            secrets,
            bin_dir: data_dir.join("bin"),
            download_url: download_url.to_string(),
            state: Default::default(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn managed_path(&self) -> PathBuf {
        self.bin_dir.join(exe_name())
    }

    /// The binary to run: `binary_path`, else the managed download, else
    /// one on `PATH`.
    fn resolve(&self, cfg: &TunnelConfig) -> Option<PathBuf> {
        if !cfg.binary_path.trim().is_empty() {
            let p = PathBuf::from(cfg.binary_path.trim());
            return p.is_file().then_some(p);
        }
        let managed = self.managed_path();
        if managed.is_file() {
            return Some(managed);
        }
        on_path(exe_name())
    }

    async fn version_of(&self, bin: &Path) -> Option<String> {
        if let Some((p, v)) = &self.lock().version {
            if p == bin {
                return Some(v.clone());
            }
        }
        let out = tokio::time::timeout(
            Duration::from_secs(10),
            Command::new(bin).arg("--version").output(),
        )
        .await
        .ok()?
        .ok()?;
        let text = String::from_utf8_lossy(&out.stdout).to_string()
            + &String::from_utf8_lossy(&out.stderr);
        let v = text.lines().next()?.trim().to_string();
        if v.is_empty() {
            return None;
        }
        self.lock().version = Some((bin.to_path_buf(), v.clone()));
        Some(v)
    }

    /// Notices a `cloudflared` that exited on its own.
    fn reap(&self) {
        let mut s = self.lock();
        let exited = match s.child.as_mut().map(|c| c.try_wait()) {
            Some(Ok(Some(status))) => Some(format!("cloudflared exited ({status})")),
            Some(Err(e)) => Some(format!("cloudflared: {e}")),
            _ => None,
        };
        if let Some(err) = exited {
            tracing::warn!("tunnel: {err}");
            s.child = None;
            s.generation += 1;
            if let Some(tx) = s.listener_stop.take() {
                let _ = tx.send(());
            }
            s.url = None;
            s.local = None;
            s.started_at = None;
            let last = s.logs.back().cloned().unwrap_or_default();
            s.error = Some(if last.is_empty() {
                err
            } else {
                format!("{err}: {last}")
            });
        }
    }

    pub async fn status(&self, cfg: &TunnelConfig) -> TunnelStatus {
        self.reap();
        let bin = self.resolve(cfg);
        let version = match &bin {
            Some(b) => self.version_of(b).await,
            None => None,
        };
        let has_token = self.has_token();
        let s = self.lock();
        TunnelStatus {
            installed: bin.is_some(),
            managed: bin.as_deref() == Some(self.managed_path().as_path()),
            binary: bin.map(|b| b.display().to_string()),
            version,
            installing: s.installing,
            running: s.child.is_some(),
            url: s.url.clone(),
            local: s.local.clone(),
            started_at: s.started_at,
            error: s.error.clone(),
            has_token,
        }
    }

    pub fn logs(&self) -> Vec<String> {
        self.lock().logs.iter().cloned().collect()
    }

    pub fn has_token(&self) -> bool {
        matches!(self.store.get_setting(TOKEN_SETTING), Ok(Some(v)) if !v.is_empty())
    }

    /// Stores (sealed) or, with `None`, removes the tunnel token.
    pub fn set_token(&self, token: Option<&str>) -> Result<()> {
        let v = match token.map(str::trim).filter(|t| !t.is_empty()) {
            Some(t) => {
                let sealed = self.secrets.seal(t.as_bytes(), TOKEN_AAD)?;
                base64::engine::general_purpose::STANDARD.encode(sealed)
            }
            None => String::new(),
        };
        self.store.set_setting(TOKEN_SETTING, &v)
    }

    fn token(&self) -> Result<Option<String>> {
        match self.store.get_setting(TOKEN_SETTING)? {
            Some(v) if !v.is_empty() => {
                let sealed = base64::engine::general_purpose::STANDARD.decode(v)?;
                let plain = self.secrets.open(&sealed, TOKEN_AAD)?;
                Ok(Some(String::from_utf8(plain)?))
            }
            _ => Ok(None),
        }
    }

    /// Downloads `cloudflared` into `data_dir/bin`.
    pub async fn install(&self, cfg: &TunnelConfig) -> Result<TunnelStatus> {
        {
            let mut s = self.lock();
            if s.installing {
                bail!("an install is already running");
            }
            s.installing = true;
        }
        let r = self.download().await;
        {
            let mut s = self.lock();
            s.installing = false;
            s.version = None;
            if let Err(e) = &r {
                s.error = Some(format!("install: {e:#}"));
            } else {
                s.error = None;
            }
        }
        r?;
        Ok(self.status(cfg).await)
    }

    async fn download(&self) -> Result<()> {
        let url = if self.download_url.is_empty() {
            default_download_url()?
        } else {
            self.download_url.clone()
        };
        tracing::info!("tunnel: downloading cloudflared from {url}");
        std::fs::create_dir_all(&self.bin_dir)?;
        let tmp = self
            .bin_dir
            .join(format!(".download-{}", std::process::id()));
        let result = async {
            let mut resp = reqwest::Client::new()
                .get(&url)
                .timeout(Duration::from_secs(600))
                .send()
                .await?
                .error_for_status()?;
            let mut f = std::fs::File::create(&tmp)?;
            while let Some(chunk) = resp.chunk().await? {
                std::io::Write::write_all(&mut f, &chunk)?;
            }
            f.sync_all()?;
            drop(f);
            let target = self.managed_path();
            if url.ends_with(".tgz") {
                let out_dir = self.bin_dir.join(".extract");
                let _ = std::fs::remove_dir_all(&out_dir);
                std::fs::create_dir_all(&out_dir)?;
                let st = Command::new("tar")
                    .arg("-xzf")
                    .arg(&tmp)
                    .arg("-C")
                    .arg(&out_dir)
                    .status()
                    .await
                    .context("run tar")?;
                if !st.success() {
                    bail!("tar failed ({st})");
                }
                std::fs::rename(out_dir.join(exe_name()), &target)
                    .context("archive has no cloudflared")?;
                let _ = std::fs::remove_dir_all(&out_dir);
                let _ = std::fs::remove_file(&tmp);
            } else {
                std::fs::rename(&tmp, &target)?;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755))?;
            }
            if self.version_of(&target).await.is_none() {
                let _ = std::fs::remove_file(&target);
                bail!("the downloaded file does not run as cloudflared");
            }
            anyhow::Ok(())
        }
        .await;
        let _ = std::fs::remove_file(&tmp);
        result.with_context(|| format!("download {url}"))
    }

    /// Starts the loopback listener serving `app` and `cloudflared` in
    /// front of it.
    pub async fn start(&self, cfg: &TunnelConfig, app: Router) -> Result<TunnelStatus> {
        self.reap();
        if self.lock().child.is_some() {
            return Ok(self.status(cfg).await);
        }
        let Some(bin) = self.resolve(cfg) else {
            bail!("cloudflared is not installed (install it here, put it on PATH, or set tunnel.binary_path)");
        };
        let token = match cfg.mode {
            TunnelMode::Token => Some(self.token()?.context("token mode needs a tunnel token")?),
            TunnelMode::Quick => None,
        };

        let listener = tokio::net::TcpListener::bind(("127.0.0.1", cfg.local_port))
            .await
            .with_context(|| format!("listen on 127.0.0.1:{} for the tunnel", cfg.local_port))?;
        let local = listener.local_addr()?.to_string();
        let (stop_tx, stop_rx) = oneshot::channel::<()>();
        tokio::spawn(async move {
            let r = axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .with_graceful_shutdown(async {
                let _ = stop_rx.await;
            })
            .await;
            if let Err(e) = r {
                tracing::warn!("tunnel listener: {e}");
            }
        });

        let mut cmd = Command::new(&bin);
        cmd.arg("tunnel");
        if cfg.protocol != "auto" {
            cmd.arg("--protocol").arg(&cfg.protocol);
        }
        match &token {
            Some(t) => {
                cmd.arg("run").env("TUNNEL_TOKEN", t);
            }
            None => {
                cmd.arg("--url").arg(format!("http://{local}"));
            }
        }
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let _ = stop_tx.send(());
                return Err(e).with_context(|| format!("run {}", bin.display()));
            }
        };

        let generation = {
            let mut s = self.lock();
            s.generation += 1;
            s.logs.clear();
            s.url = None;
            s.error = None;
            s.local = Some(local.clone());
            s.started_at = Some(crate::now_unix());
            s.listener_stop = Some(stop_tx);
            if let Some(out) = child.stdout.take() {
                self.read_lines(out, s.generation);
            }
            if let Some(err) = child.stderr.take() {
                self.read_lines(err, s.generation);
            }
            s.child = Some(child);
            s.generation
        };
        tracing::info!(
            "tunnel: started {} ({:?} mode) → {local}",
            bin.display(),
            cfg.mode
        );
        // Notice an early exit without anyone polling the status.
        let me = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(2)).await;
                if me.lock().generation != generation {
                    break;
                }
                me.reap();
            }
        });
        Ok(self.status(cfg).await)
    }

    fn read_lines<R: AsyncRead + Unpin + Send + 'static>(&self, stream: R, generation: u64) {
        let me = self.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stream).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!(target: "cloudflared", "{line}");
                let mut s = me.lock();
                if s.generation != generation {
                    break;
                }
                if s.url.is_none() {
                    if let Some(u) = extract_url(&line) {
                        tracing::info!("tunnel: public URL {u}");
                        s.url = Some(u);
                    }
                }
                if s.logs.len() >= LOG_LINES {
                    s.logs.pop_front();
                }
                s.logs.push_back(line);
            }
        });
    }

    /// Stops `cloudflared` and the tunnel listener.
    pub fn stop(&self) {
        let mut s = self.lock();
        s.generation += 1;
        if let Some(mut c) = s.child.take() {
            let _ = c.start_kill();
            tracing::info!("tunnel: stopped");
        }
        if let Some(tx) = s.listener_stop.take() {
            let _ = tx.send(());
        }
        s.url = None;
        s.local = None;
        s.started_at = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_from_quick_and_named_tunnels() {
        let quick = "2026-10-25T08:00:00Z INF |  https://sharp-owl-42.trycloudflare.com                                     |";
        assert_eq!(
            extract_url(quick).as_deref(),
            Some("https://sharp-owl-42.trycloudflare.com")
        );
        let named = r#"2026-10-25T08:00:01Z INF Updated to new configuration config="{\"ingress\":[{\"hostname\":\"cc.example.com\", \"service\":\"http://localhost:8788\"}, {\"service\":\"http_status:404\"}]}" version=3"#;
        assert_eq!(
            extract_url(named).as_deref(),
            Some("https://cc.example.com")
        );
        assert_eq!(extract_url("INF Starting tunnel tunnelID=abc"), None);
        assert_eq!(
            extract_url("Requesting new quick Tunnel on trycloudflare.com..."),
            None
        );
    }

    #[test]
    fn token_is_sealed_at_rest() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory().unwrap();
        let secrets = Arc::new(SecretBox::new(&[7u8; 32]).unwrap());
        let t = Tunnel::new(store.clone(), secrets, dir.path(), "");
        assert!(!t.has_token());
        t.set_token(Some("eyJhIjoiYiJ9")).unwrap();
        assert!(t.has_token());
        let raw = store.get_setting(TOKEN_SETTING).unwrap().unwrap();
        assert!(!raw.contains("eyJhIjoiYiJ9"));
        assert_eq!(t.token().unwrap().as_deref(), Some("eyJhIjoiYiJ9"));
        t.set_token(None).unwrap();
        assert!(!t.has_token() && t.token().unwrap().is_none());
    }
}
