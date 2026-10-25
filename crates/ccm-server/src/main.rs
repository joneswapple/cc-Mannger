//! Headless CC Manager: the same core as the desktop app, without a window,
//! plus admin / maintenance subcommands.

mod admin;
mod migrate;
mod ui;

use std::io::Read;
use std::path::PathBuf;

use anyhow::{bail, Result};
use ccm_core::{auth::Admin, config::Config, store::Store, Core};
use clap::{Args, Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "ccm-server",
    version,
    about = "Share Claude Pro/Max OAuth accounts with your terminals"
)]
struct Cli {
    /// Path to config.json. Missing file means defaults + CCM_* env vars.
    #[arg(long, global = true, env = "CCM_CONFIG")]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the proxy server (default).
    Serve,
    /// Print the effective configuration as JSON (secrets redacted).
    Config,
    /// Admin operations against a running server's admin API
    /// (CCM_ADMIN_URL, CCM_ADMIN_TOKEN), except `reset-token`.
    Admin(admin::AdminArgs),
    /// Inspect / dry-run effort-tier expansion.
    #[command(subcommand)]
    Effort(EffortCommand),
    /// Import users, keys, invites, policies, usage and accounts from a Go
    /// cliproxy database into this (empty) one.
    MigrateFromGo(migrate::MigrateArgs),
}

#[derive(Subcommand)]
enum EffortCommand {
    /// Print the configured effort tiers.
    List,
    /// Show the rewritten request body for a tier, a --body, or stdin.
    Expand(ExpandArgs),
}

#[derive(Args)]
struct ExpandArgs {
    /// Effort tier name (auto/low/medium/high/xhigh/max).
    #[arg(long)]
    tier: Option<String>,
    /// Raw JSON body to expand (overrides --tier).
    #[arg(long)]
    body: Option<String>,
    /// Print compact JSON instead of pretty-printing.
    #[arg(long)]
    compact: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let cfg = Config::load(cli.config.as_deref())?;
    {
        use tracing_subscriber::prelude::*;
        // Console output plus the in-memory buffer behind the UI log viewer.
        tracing_subscriber::registry()
            .with(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&cfg.log_level)),
            )
            .with(tracing_subscriber::fmt::layer())
            .with(ccm_core::logbuf::layer())
            .init();
    }

    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => serve(cfg).await,
        Command::Config => {
            let mut shown = cfg;
            for s in [&mut shown.secret_key, &mut shown.admin.initial_token] {
                if !s.is_empty() {
                    *s = "<redacted>".into();
                }
            }
            println!("{}", ccm_core::config::to_pretty_json(&shown)?);
            Ok(())
        }
        Command::Admin(args) => {
            if let admin::AdminCommand::ResetToken { token } = &args.command {
                return reset_token(&cfg, token.as_deref().unwrap_or(""));
            }
            admin::run(args).await
        }
        Command::Effort(EffortCommand::List) => {
            println!(
                "{:<8}  {:>10}  {:>15}",
                "TIER", "MAX_TOKENS", "THINKING_BUDGET"
            );
            for (name, lvl) in &cfg.upstream.effort_levels {
                let budget = if lvl.thinking_budget > 0 {
                    lvl.thinking_budget.to_string()
                } else {
                    "—".into()
                };
                println!("{name:<8}  {:>10}  {budget:>15}", lvl.max_tokens);
            }
            Ok(())
        }
        Command::Effort(EffortCommand::Expand(a)) => expand(&cfg, a),
        Command::MigrateFromGo(args) => migrate::run(&cfg, args),
    }
}

async fn serve(cfg: Config) -> Result<()> {
    let (core, startup) = Core::open(cfg)?;
    if let Some(tok) = core.admin.bootstrap(&core.config.admin.initial_token)? {
        if core.config.admin.initial_token.is_empty() {
            println!("\n=== ADMIN TOKEN (save now, only shown once) ===\n{tok}\n===============================================\n");
        }
    }
    if let Some(id) = startup.imported_account {
        println!("imported Claude Code credentials as upstream account #{id}");
    }
    if let Some(e) = &startup.import_error {
        if core.accounts.count()? == 0 {
            eprintln!(
                "warning: no upstream accounts yet ({e}); requests will fail until one is added"
            );
        }
    }
    println!("upstream credentials: {}", core.pool.source());
    let tunnel = core.tunnel.clone();
    let r = ccm_core::http::serve_with(core, ui::router(), async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await;
    tunnel.stop();
    r
}

/// Installs a new admin token directly in the local database (works without
/// the server; a running server picks it up within a minute).
fn reset_token(cfg: &Config, preset: &str) -> Result<()> {
    let tok = Admin::new(Store::open(&cfg.data_dir)?).reset(preset)?;
    println!("\n=== NEW ADMIN TOKEN ===\n{tok}\n=======================");
    Ok(())
}

fn expand(cfg: &Config, a: ExpandArgs) -> Result<()> {
    let input = match (a.body, a.tier) {
        (Some(b), _) => b.into_bytes(),
        (None, Some(t)) => serde_json::to_vec(&serde_json::json!({"effort": t}))?,
        (None, None) => {
            let mut buf = Vec::new();
            std::io::stdin().take(16 << 20).read_to_end(&mut buf)?;
            if buf.iter().all(u8::is_ascii_whitespace) {
                bail!("no input — pass --tier, --body, or pipe a JSON body on stdin");
            }
            buf
        }
    };
    let out = match ccm_core::rewrite::apply_effort(&input, &cfg.upstream.effort_levels)? {
        Some(out) => out,
        None => {
            eprintln!("(no effort field — body forwarded unchanged)");
            input
        }
    };
    match serde_json::from_slice::<serde_json::Value>(&out) {
        Ok(v) if !a.compact => println!("{}", serde_json::to_string_pretty(&v)?),
        _ => println!("{}", String::from_utf8_lossy(&out)),
    }
    Ok(())
}
