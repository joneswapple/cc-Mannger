//! `ccm-server admin …`: thin wrappers over the running server's
//! `/admin/api/*` endpoints.

use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};
use serde_json::{json, Value};

#[derive(Args)]
pub struct AdminArgs {
    /// Admin API base URL.
    #[arg(
        long,
        env = "CCM_ADMIN_URL",
        default_value = "http://127.0.0.1:8787",
        global = true
    )]
    url: String,
    /// Admin token.
    #[arg(long, env = "CCM_ADMIN_TOKEN", hide_env_values = true, global = true)]
    token: Option<String>,
    #[command(subcommand)]
    pub command: AdminCommand,
}

#[derive(Subcommand)]
pub enum AdminCommand {
    /// Server, account and model-catalog status.
    Status,
    #[command(subcommand)]
    Invite(InviteCommand),
    #[command(subcommand)]
    User(UserCommand),
    #[command(subcommand)]
    Key(KeyCommand),
    #[command(subcommand)]
    Account(AccountCommand),
    #[command(subcommand)]
    Models(ModelsCommand),
    /// Usage rollups by day, user, model and account.
    Stats {
        #[arg(long, default_value_t = 30)]
        days: i64,
    },
    /// Search usage events.
    Usage {
        /// Free text matched against model, user label and error.
        #[arg(long)]
        q: Option<String>,
        #[arg(long)]
        user_id: Option<i64>,
        #[arg(long)]
        model: Option<String>,
        /// ok | error | 2xx | 4xx | 5xx | <code>
        #[arg(long)]
        status: Option<String>,
        #[arg(long, default_value_t = 1)]
        page: i64,
        #[arg(long, default_value_t = 50)]
        page_size: i64,
    },
    /// Usage overview: series, rollups with estimated cost, heatmap.
    Overview {
        #[arg(long, default_value_t = 7)]
        days: i64,
        /// hour | day | week
        #[arg(long, default_value = "day")]
        granularity: String,
        /// none | model | account | user
        #[arg(long, default_value = "none")]
        group: String,
        #[arg(long)]
        user_id: Option<i64>,
    },
    /// Show the price table used for cost estimates, or replace it with a
    /// JSON array of {pattern,input,output,cache_read,cache_write} (USD per
    /// million tokens); '[]' restores the defaults.
    Prices { set: Option<String> },
    /// Show runtime settings, or apply a JSON merge patch to them
    /// (e.g. '{"pool":{"strategy":"least_used"}}'; null resets a field).
    Settings { patch: Option<String> },
    /// Proxy listener status, or start / stop / restart it (desktop app only).
    Service {
        #[arg(value_parser = ["status", "start", "stop", "restart"], default_value = "status")]
        action: String,
    },
    #[command(subcommand)]
    Monitor(MonitorCommand),
    /// Send a tiny request to a model through the pool (connectivity test).
    TestModel { model: String },
    #[command(subcommand)]
    ClaudeCode(ClaudeCodeCommand),
    #[command(subcommand)]
    Security(SecurityCommand),
    #[command(subcommand)]
    Proxy(ProxyCommand),
    #[command(subcommand)]
    Tunnel(TunnelCommand),
    /// Recent server log lines.
    Logs {
        /// ERROR | WARN | INFO | DEBUG
        #[arg(long, default_value = "INFO")]
        level: String,
        #[arg(long, default_value_t = 200)]
        limit: usize,
    },
    /// Compare this build with the latest GitHub release.
    UpdateCheck,
    /// Version, platform and data paths.
    System,
    /// Reset the admin token in the local database (no server needed).
    ResetToken {
        /// Explicit token to install (default: random).
        #[arg(long)]
        token: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum MonitorCommand {
    /// Recent requests (newest first).
    List {
        #[arg(long)]
        q: Option<String>,
        /// ok | error | <code>
        #[arg(long)]
        status: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// One request with its bodies.
    Show {
        id: u64,
    },
    /// Start recording requests.
    On,
    /// Stop recording requests.
    Off,
    Clear,
}

/// IP rules and the access log for `/v1/*` and `/register`.
#[derive(Subcommand)]
pub enum SecurityCommand {
    Rules,
    /// Allow an IP or CIDR (any allow rule turns on allowlist mode).
    Allow {
        cidr: String,
        #[arg(long, default_value = "")]
        note: String,
        /// Lapse after this many hours (default: permanent).
        #[arg(long)]
        hours: Option<i64>,
    },
    /// Block an IP or CIDR.
    Deny {
        cidr: String,
        #[arg(long, default_value = "")]
        note: String,
        #[arg(long)]
        hours: Option<i64>,
    },
    DeleteRule {
        id: i64,
    },
    /// Access log, newest first.
    Log {
        #[arg(long)]
        ip: Option<String>,
        /// Only blocked requests.
        #[arg(long)]
        blocked: bool,
        #[arg(long, default_value_t = 50)]
        limit: i64,
    },
    /// Per-IP totals.
    Ips {
        #[arg(long, default_value_t = 7)]
        days: i64,
    },
}

/// Public access through a Cloudflare tunnel.
#[derive(Subcommand)]
pub enum TunnelCommand {
    Status,
    /// Download cloudflared into the data directory.
    Install,
    Start,
    Stop,
    /// Save the tunnel token for token mode (`-` reads it from stdin).
    Token {
        token: String,
    },
    ClearToken,
    /// Recent cloudflared output.
    Logs,
    /// Change tunnel settings (applied on the next start).
    Set {
        /// quick | token
        #[arg(long)]
        mode: Option<String>,
        /// auto | http2 | quic
        #[arg(long)]
        protocol: Option<String>,
        /// Loopback port the tunnel forwards to.
        #[arg(long)]
        port: Option<u16>,
        /// Also serve the admin API and web UI through the tunnel.
        #[arg(long)]
        expose_admin: Option<bool>,
        #[arg(long)]
        auto_start: Option<bool>,
    },
}

/// The shared upstream proxy list.
#[derive(Subcommand)]
pub enum ProxyCommand {
    List,
    Add {
        url: String,
        #[arg(long, default_value = "")]
        name: String,
    },
    Delete {
        id: i64,
    },
    /// Reach the upstream through the proxy.
    Test {
        id: i64,
    },
    /// Route exactly these accounts through the proxy.
    Assign {
        id: i64,
        #[arg(value_delimiter = ',')]
        accounts: Vec<i64>,
    },
}

/// The server machine's `~/.claude/settings.json`.
#[derive(Subcommand)]
pub enum ClaudeCodeCommand {
    Status,
    /// Point Claude Code at this server (issues a key for it).
    Sync {
        /// URL Claude Code should use, e.g. http://127.0.0.1:8787.
        #[arg(long)]
        base_url: String,
    },
    /// Put the original settings file back.
    Restore,
}

#[derive(Subcommand)]
pub enum InviteCommand {
    /// Create an invite token (shown once).
    Create {
        #[arg(long)]
        label: String,
        #[arg(long, default_value_t = 24)]
        ttl_hours: i64,
    },
    List,
    /// Delete an unredeemed invite by its token hash.
    Delete {
        hash: String,
    },
}

#[derive(Subcommand)]
pub enum UserCommand {
    List,
    Create {
        #[arg(long)]
        label: String,
    },
    Enable {
        id: i64,
    },
    Disable {
        id: i64,
    },
    /// Show, or replace with the given flags, a user's policy.
    Policy {
        id: i64,
        /// Comma-separated model allowlist; empty string clears it.
        #[arg(long)]
        allow: Option<String>,
        #[arg(long)]
        daily_tokens: Option<i64>,
        #[arg(long)]
        rpm: Option<i64>,
        /// Only use upstream accounts in this group; empty string clears it.
        #[arg(long)]
        group: Option<String>,
        /// Distinct client IPs allowed per 24 hours.
        #[arg(long)]
        max_ips: Option<i64>,
    },
    /// Client IPs a user's keys have been used from.
    Ips {
        id: i64,
    },
}

#[derive(Subcommand)]
pub enum KeyCommand {
    /// Issue a new key for a user (shown once).
    Issue {
        user_id: i64,
        /// Expire the key after this many days (default: never).
        #[arg(long)]
        ttl_days: Option<i64>,
    },
    /// Set a key to expire this many days from now (0 = never).
    Renew {
        id: i64,
        #[arg(long)]
        days: i64,
    },
    Revoke {
        id: i64,
    },
}

#[derive(Subcommand)]
pub enum AccountCommand {
    List,
    /// Change scheduling settings of an upstream account.
    Set {
        id: i64,
        #[arg(long)]
        label: Option<String>,
        /// Higher is preferred by the `priority` strategy.
        #[arg(long)]
        priority: Option<i64>,
        /// Share under the `round_robin` strategy.
        #[arg(long)]
        weight: Option<i64>,
        /// Account group; empty string removes it.
        #[arg(long)]
        group: Option<String>,
        #[arg(long, conflicts_with = "disable")]
        enable: bool,
        #[arg(long)]
        disable: bool,
        /// Lift an active cooldown.
        #[arg(long)]
        clear_cooldown: bool,
        /// Outbound proxy (http://, socks5://…); empty string removes it.
        #[arg(long)]
        proxy: Option<String>,
    },
    /// Log in a Claude account with OAuth: prints a URL, then asks for the
    /// `code#state` shown after signing in.
    Login {
        #[arg(long, default_value = "")]
        label: String,
        #[arg(long, default_value = "")]
        group: String,
    },
    /// Import credentials: a Claude Code `.credentials.json`, a JSON array
    /// of them, or the macOS keychain entry.
    Import {
        /// JSON file to import (`-` for stdin).
        #[arg(long, conflicts_with = "from_keychain")]
        file: Option<String>,
        /// Read Claude Code's credentials from the macOS keychain.
        #[arg(long)]
        from_keychain: bool,
        #[arg(long, default_value = "")]
        label: String,
        #[arg(long, default_value = "")]
        group: String,
    },
    /// Print accounts with their tokens as JSON (importable with
    /// `account import --file`). Handle the output as a secret.
    Export {
        /// Account ids; none exports all.
        ids: Vec<i64>,
    },
    /// Refresh the account's token now and re-read its profile.
    Refresh {
        id: i64,
    },
    /// Move the account to the top of the priority order.
    Pin {
        id: i64,
    },
    /// Delete the account and its stored tokens.
    Delete {
        id: i64,
    },
}

#[derive(Subcommand)]
pub enum ModelsCommand {
    /// Show settings, the model catalog and the global denylist.
    List,
    /// Replace the global model denylist.
    Disable { models: Vec<String> },
    /// Refetch the model catalog from upstream.
    Refresh,
    /// Replace the model routes: `pattern=target` pairs in match order
    /// (`re:` prefix for a regex). No pairs clears them.
    Routes { routes: Vec<String> },
}

pub async fn run(args: AdminArgs) -> Result<()> {
    let token = args
        .token
        .context("set CCM_ADMIN_TOKEN (or --token) to use admin subcommands")?;
    let c = Client {
        http: reqwest::Client::new(),
        base: args.url.trim_end_matches('/').to_string(),
        token,
    };
    let out = match args.command {
        AdminCommand::Status => c.get("/admin/api/status").await?,
        AdminCommand::Invite(InviteCommand::Create { label, ttl_hours }) => {
            c.post(
                "/admin/api/invites",
                json!({"label": label, "ttl_hours": ttl_hours}),
            )
            .await?
        }
        AdminCommand::Invite(InviteCommand::List) => c.get("/admin/api/invites").await?,
        AdminCommand::Invite(InviteCommand::Delete { hash }) => {
            c.post(&format!("/admin/api/invites/{hash}/delete"), Value::Null)
                .await?
        }
        AdminCommand::User(UserCommand::List) => c.get("/admin/api/users").await?,
        AdminCommand::User(UserCommand::Create { label }) => {
            c.post("/admin/api/users", json!({"label": label})).await?
        }
        AdminCommand::User(UserCommand::Enable { id }) => {
            c.post(&format!("/admin/api/users/{id}/enable"), Value::Null)
                .await?
        }
        AdminCommand::User(UserCommand::Disable { id }) => {
            c.post(&format!("/admin/api/users/{id}/disable"), Value::Null)
                .await?
        }
        AdminCommand::User(UserCommand::Policy {
            id,
            allow,
            daily_tokens,
            rpm,
            group,
            max_ips,
        }) => {
            let path = format!("/admin/api/users/{id}/policy");
            if allow.is_none()
                && daily_tokens.is_none()
                && rpm.is_none()
                && group.is_none()
                && max_ips.is_none()
            {
                c.get(&path).await?
            } else {
                let allowed: Option<Vec<String>> =
                    allow.filter(|a| !a.trim().is_empty()).map(|a| {
                        a.split(',')
                            .map(|m| m.trim().to_string())
                            .filter(|m| !m.is_empty())
                            .collect()
                    });
                c.post(&path, json!({"allowed_models": allowed, "daily_token_limit": daily_tokens, "rpm_limit": rpm, "account_group": group, "max_ips": max_ips}))
                    .await?
            }
        }
        AdminCommand::User(UserCommand::Ips { id }) => {
            c.get(&format!("/admin/api/users/{id}/ips")).await?
        }
        AdminCommand::Key(KeyCommand::Issue { user_id, ttl_days }) => {
            c.post(
                "/admin/api/keys/issue",
                json!({"user_id": user_id, "ttl_days": ttl_days.unwrap_or(0)}),
            )
            .await?
        }
        AdminCommand::Key(KeyCommand::Renew { id, days }) => {
            c.post(
                &format!("/admin/api/keys/{id}/expiry"),
                json!({"days": days}),
            )
            .await?
        }
        AdminCommand::Key(KeyCommand::Revoke { id }) => {
            c.post(&format!("/admin/api/keys/{id}/revoke"), Value::Null)
                .await?
        }
        AdminCommand::Account(AccountCommand::List) => c.get("/admin/api/accounts").await?,
        AdminCommand::Account(AccountCommand::Set {
            id,
            label,
            priority,
            weight,
            group,
            enable,
            disable,
            clear_cooldown,
            proxy,
        }) => {
            let enabled = match (enable, disable) {
                (true, _) => Some(true),
                (_, true) => Some(false),
                _ => None,
            };
            c.post(
                &format!("/admin/api/accounts/{id}"),
                json!({
                    "label": label,
                    "priority": priority,
                    "weight": weight,
                    "account_group": group,
                    "enabled": enabled,
                    "clear_cooldown": clear_cooldown,
                    "proxy_url": proxy,
                }),
            )
            .await?
        }
        AdminCommand::Account(AccountCommand::Login { label, group }) => {
            let start = c
                .post(
                    "/admin/api/accounts/oauth/start",
                    json!({"label": label, "account_group": group}),
                )
                .await?;
            println!(
                "Open this URL in a browser signed in to the Claude account:\n\n{}\n",
                start["url"].as_str().unwrap_or_default()
            );
            eprint!("Paste the code shown after signing in (code#state): ");
            let mut code = String::new();
            std::io::stdin().read_line(&mut code)?;
            c.post(
                "/admin/api/accounts/oauth/complete",
                json!({"code": code.trim(), "state": start["state"]}),
            )
            .await?
        }
        AdminCommand::Account(AccountCommand::Import {
            file,
            from_keychain,
            label,
            group,
        }) => {
            let text = if from_keychain {
                read_keychain()?
            } else {
                match file.as_deref() {
                    Some("-") | None => {
                        let mut s = String::new();
                        std::io::Read::read_to_string(&mut std::io::stdin(), &mut s)?;
                        s
                    }
                    Some(path) => {
                        std::fs::read_to_string(path).with_context(|| format!("read {path}"))?
                    }
                }
            };
            let credentials: Value =
                serde_json::from_str(&text).context("credentials are not valid JSON")?;
            c.post(
                "/admin/api/accounts/import",
                json!({"credentials": credentials, "label": label, "account_group": group}),
            )
            .await?
        }
        AdminCommand::Account(AccountCommand::Export { ids }) => {
            let ids: Vec<String> = ids.iter().map(|i| i.to_string()).collect();
            c.get_query("/admin/api/accounts/export", &[("ids", ids.join(","))])
                .await?
        }
        AdminCommand::Account(AccountCommand::Refresh { id }) => {
            c.post(&format!("/admin/api/accounts/{id}/refresh"), Value::Null)
                .await?
        }
        AdminCommand::Account(AccountCommand::Pin { id }) => {
            c.post(&format!("/admin/api/accounts/{id}/pin"), Value::Null)
                .await?
        }
        AdminCommand::Account(AccountCommand::Delete { id }) => {
            c.post(&format!("/admin/api/accounts/{id}/delete"), Value::Null)
                .await?
        }
        AdminCommand::Models(ModelsCommand::List) => c.get("/admin/api/settings").await?,
        AdminCommand::Models(ModelsCommand::Disable { models }) => {
            c.post(
                "/admin/api/settings/disabled-models",
                json!({"models": models}),
            )
            .await?
        }
        AdminCommand::Models(ModelsCommand::Routes { routes }) => {
            let routes: Vec<Value> = routes
                .iter()
                .map(|r| {
                    r.split_once('=')
                        .map(|(p, t)| json!({"pattern": p.trim(), "target": t.trim()}))
                        .with_context(|| format!("route {r:?} is not pattern=target"))
                })
                .collect::<Result<_>>()?;
            c.post(
                "/admin/api/settings/model-routes",
                json!({"routes": routes}),
            )
            .await?
        }
        AdminCommand::Models(ModelsCommand::Refresh) => {
            c.post("/admin/api/settings/refresh-models", Value::Null)
                .await?
        }
        AdminCommand::Stats { days } => c.get(&format!("/admin/api/stats?days={days}")).await?,
        AdminCommand::Usage {
            q,
            user_id,
            model,
            status,
            page,
            page_size,
        } => {
            let mut query = vec![
                ("page", page.to_string()),
                ("page_size", page_size.to_string()),
            ];
            query.extend(q.map(|v| ("q", v)));
            query.extend(user_id.map(|v| ("user_id", v.to_string())));
            query.extend(model.map(|v| ("model", v)));
            query.extend(status.map(|v| ("status", v)));
            c.get_query("/admin/api/usage", &query).await?
        }
        AdminCommand::Overview {
            days,
            granularity,
            group,
            user_id,
        } => {
            let mut q = vec![
                ("days", days.to_string()),
                ("granularity", granularity),
                ("group", group),
            ];
            q.extend(user_id.map(|u| ("user_id", u.to_string())));
            c.get_query("/admin/api/stats/overview", &q).await?
        }
        AdminCommand::Prices { set: None } => c.get("/admin/api/settings/prices").await?,
        AdminCommand::Prices {
            set: Some(json_text),
        } => {
            let prices: Value =
                serde_json::from_str(&json_text).context("prices must be a JSON array")?;
            c.post("/admin/api/settings/prices", json!({ "prices": prices }))
                .await?
        }
        AdminCommand::Settings { patch: None } => c.get("/admin/api/runtime").await?,
        AdminCommand::Settings { patch: Some(p) } => {
            let patch: Value = serde_json::from_str(&p).context("patch must be JSON")?;
            c.post("/admin/api/runtime", patch).await?
        }
        AdminCommand::Service { action } => match action.as_str() {
            "status" => c.get("/admin/api/service").await?,
            a => {
                c.post(&format!("/admin/api/service/{a}"), Value::Null)
                    .await?
            }
        },
        AdminCommand::Monitor(MonitorCommand::List { q, status, limit }) => {
            let mut query = vec![("limit", limit.to_string())];
            query.extend(q.map(|v| ("q", v)));
            query.extend(status.map(|v| ("status", v)));
            c.get_query("/admin/api/monitor", &query).await?
        }
        AdminCommand::Monitor(MonitorCommand::Show { id }) => {
            c.get(&format!("/admin/api/monitor/{id}")).await?
        }
        AdminCommand::Monitor(MonitorCommand::On) => {
            c.post("/admin/api/runtime", json!({"monitor": {"enabled": true}}))
                .await?
        }
        AdminCommand::Monitor(MonitorCommand::Off) => {
            c.post("/admin/api/runtime", json!({"monitor": {"enabled": false}}))
                .await?
        }
        AdminCommand::Monitor(MonitorCommand::Clear) => {
            c.post("/admin/api/monitor/clear", Value::Null).await?
        }
        AdminCommand::TestModel { model } => {
            c.post("/admin/api/proxy/test", json!({ "model": model }))
                .await?
        }
        AdminCommand::ClaudeCode(ClaudeCodeCommand::Status) => {
            c.get("/admin/api/claude-code").await?
        }
        AdminCommand::ClaudeCode(ClaudeCodeCommand::Sync { base_url }) => {
            c.post(
                "/admin/api/claude-code/sync",
                json!({ "base_url": base_url }),
            )
            .await?
        }
        AdminCommand::ClaudeCode(ClaudeCodeCommand::Restore) => {
            c.post("/admin/api/claude-code/restore", Value::Null)
                .await?
        }
        AdminCommand::Security(SecurityCommand::Rules) => {
            c.get("/admin/api/security/rules").await?
        }
        AdminCommand::Security(SecurityCommand::Allow { cidr, note, hours }) => {
            let body = json!({"kind": "allow", "cidr": cidr, "note": note, "expires_hours": hours.unwrap_or(0)});
            c.post("/admin/api/security/rules", body).await?
        }
        AdminCommand::Security(SecurityCommand::Deny { cidr, note, hours }) => {
            let body = json!({"kind": "deny", "cidr": cidr, "note": note, "expires_hours": hours.unwrap_or(0)});
            c.post("/admin/api/security/rules", body).await?
        }
        AdminCommand::Security(SecurityCommand::DeleteRule { id }) => {
            c.post(
                &format!("/admin/api/security/rules/{id}/delete"),
                Value::Null,
            )
            .await?
        }
        AdminCommand::Security(SecurityCommand::Log { ip, blocked, limit }) => {
            let mut q = vec![("page_size", limit.to_string())];
            q.extend(ip.map(|v| ("ip", v)));
            if blocked {
                q.push(("filter", "blocked".into()));
            }
            c.get_query("/admin/api/security/log", &q).await?
        }
        AdminCommand::Security(SecurityCommand::Ips { days }) => {
            c.get(&format!("/admin/api/security/ips?days={days}"))
                .await?
        }
        AdminCommand::Proxy(ProxyCommand::List) => c.get("/admin/api/proxies").await?,
        AdminCommand::Proxy(ProxyCommand::Add { url, name }) => {
            c.post("/admin/api/proxies", json!({"url": url, "name": name}))
                .await?
        }
        AdminCommand::Proxy(ProxyCommand::Delete { id }) => {
            c.post(&format!("/admin/api/proxies/{id}/delete"), Value::Null)
                .await?
        }
        AdminCommand::Proxy(ProxyCommand::Test { id }) => {
            c.post(&format!("/admin/api/proxies/{id}/test"), Value::Null)
                .await?
        }
        AdminCommand::Proxy(ProxyCommand::Assign { id, accounts }) => {
            c.post(
                &format!("/admin/api/proxies/{id}/assign"),
                json!({ "account_ids": accounts }),
            )
            .await?
        }
        AdminCommand::Tunnel(TunnelCommand::Status) => c.get("/admin/api/tunnel").await?,
        AdminCommand::Tunnel(TunnelCommand::Install) => {
            c.post("/admin/api/tunnel/install", Value::Null).await?
        }
        AdminCommand::Tunnel(TunnelCommand::Start) => {
            c.post("/admin/api/tunnel/start", Value::Null).await?
        }
        AdminCommand::Tunnel(TunnelCommand::Stop) => {
            c.post("/admin/api/tunnel/stop", Value::Null).await?
        }
        AdminCommand::Tunnel(TunnelCommand::Token { token }) => {
            let token = if token == "-" {
                let mut s = String::new();
                std::io::stdin().read_line(&mut s)?;
                s.trim().to_string()
            } else {
                token
            };
            c.post("/admin/api/tunnel/token", json!({ "token": token }))
                .await?
        }
        AdminCommand::Tunnel(TunnelCommand::ClearToken) => {
            c.post("/admin/api/tunnel/token", json!({ "token": null }))
                .await?
        }
        AdminCommand::Tunnel(TunnelCommand::Logs) => c.get("/admin/api/tunnel/logs").await?,
        AdminCommand::Tunnel(TunnelCommand::Set {
            mode,
            protocol,
            port,
            expose_admin,
            auto_start,
        }) => {
            let mut t = serde_json::Map::new();
            for (k, v) in [
                ("mode", mode.map(Value::from)),
                ("protocol", protocol.map(Value::from)),
                ("local_port", port.map(Value::from)),
                ("expose_admin", expose_admin.map(Value::from)),
                ("auto_start", auto_start.map(Value::from)),
            ] {
                if let Some(v) = v {
                    t.insert(k.into(), v);
                }
            }
            c.post("/admin/api/runtime", json!({ "tunnel": t })).await?
        }
        AdminCommand::Logs { level, limit } => {
            c.get_query(
                "/admin/api/logs",
                &[("level", level), ("limit", limit.to_string())],
            )
            .await?
        }
        AdminCommand::UpdateCheck => c.get("/admin/api/update/check").await?,
        AdminCommand::System => c.get("/admin/api/system").await?,
        AdminCommand::ResetToken { .. } => unreachable!("handled locally"),
    };
    match out {
        Value::Null => println!("ok"),
        v => println!("{}", serde_json::to_string_pretty(&v)?),
    }
    Ok(())
}

/// Claude Code's credentials from the macOS keychain (it keeps them there
/// instead of `~/.claude/.credentials.json` on macOS).
fn read_keychain() -> Result<String> {
    if !cfg!(target_os = "macos") {
        bail!("--from-keychain only works on macOS; use --file ~/.claude/.credentials.json");
    }
    let out = std::process::Command::new("security")
        .args([
            "find-generic-password",
            "-s",
            "Claude Code-credentials",
            "-w",
        ])
        .output()
        .context("run `security`")?;
    if !out.status.success() {
        bail!(
            "keychain lookup failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8(out.stdout)?)
}

struct Client {
    http: reqwest::Client,
    base: String,
    token: String,
}

impl Client {
    async fn get(&self, path: &str) -> Result<Value> {
        self.send(self.http.get(format!("{}{path}", self.base)))
            .await
    }

    async fn get_query(&self, path: &str, query: &[(&str, String)]) -> Result<Value> {
        self.send(self.http.get(format!("{}{path}", self.base)).query(query))
            .await
    }

    async fn post(&self, path: &str, body: Value) -> Result<Value> {
        let mut req = self.http.post(format!("{}{path}", self.base));
        if !body.is_null() {
            req = req.json(&body);
        }
        self.send(req).await
    }

    async fn send(&self, req: reqwest::RequestBuilder) -> Result<Value> {
        let resp = req
            .bearer_auth(&self.token)
            .timeout(std::time::Duration::from_secs(30))
            .send()
            .await
            .with_context(|| format!("connect to {}", self.base))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!("{} -> {}", status.as_u16(), text);
        }
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        Ok(serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }
}
