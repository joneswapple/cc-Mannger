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
    /// Reset the admin token in the local database (no server needed).
    ResetToken {
        /// Explicit token to install (default: random).
        #[arg(long)]
        token: Option<String>,
    },
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
    },
}

#[derive(Subcommand)]
pub enum KeyCommand {
    /// Issue a new key for a user (shown once).
    Issue {
        user_id: i64,
    },
    Revoke {
        id: i64,
    },
}

#[derive(Subcommand)]
pub enum AccountCommand {
    List,
}

#[derive(Subcommand)]
pub enum ModelsCommand {
    /// Show settings, the model catalog and the global denylist.
    List,
    /// Replace the global model denylist.
    Disable { models: Vec<String> },
    /// Refetch the model catalog from upstream.
    Refresh,
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
        }) => {
            let path = format!("/admin/api/users/{id}/policy");
            if allow.is_none() && daily_tokens.is_none() && rpm.is_none() {
                c.get(&path).await?
            } else {
                let allowed: Option<Vec<String>> =
                    allow.filter(|a| !a.trim().is_empty()).map(|a| {
                        a.split(',')
                            .map(|m| m.trim().to_string())
                            .filter(|m| !m.is_empty())
                            .collect()
                    });
                c.post(&path, json!({"allowed_models": allowed, "daily_token_limit": daily_tokens, "rpm_limit": rpm}))
                    .await?
            }
        }
        AdminCommand::Key(KeyCommand::Issue { user_id }) => {
            c.post("/admin/api/keys/issue", json!({"user_id": user_id}))
                .await?
        }
        AdminCommand::Key(KeyCommand::Revoke { id }) => {
            c.post(&format!("/admin/api/keys/{id}/revoke"), Value::Null)
                .await?
        }
        AdminCommand::Account(AccountCommand::List) => c.get("/admin/api/accounts").await?,
        AdminCommand::Models(ModelsCommand::List) => c.get("/admin/api/settings").await?,
        AdminCommand::Models(ModelsCommand::Disable { models }) => {
            c.post(
                "/admin/api/settings/disabled-models",
                json!({"models": models}),
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
        AdminCommand::ResetToken { .. } => unreachable!("handled locally"),
    };
    match out {
        Value::Null => println!("ok"),
        v => println!("{}", serde_json::to_string_pretty(&v)?),
    }
    Ok(())
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
