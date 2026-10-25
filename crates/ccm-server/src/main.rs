//! Headless CC Manager: the same core as the desktop app, without a window.

use std::path::PathBuf;

use anyhow::Result;
use ccm_core::{config::Config, Core};
use clap::{Parser, Subcommand};

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
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let cfg = Config::load(cli.config.as_deref())?;
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&cfg.log_level)),
        )
        .init();

    match cli.command.unwrap_or(Command::Serve) {
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
        Command::Serve => serve(cfg).await,
    }
}

async fn serve(cfg: Config) -> Result<()> {
    let (core, startup) = Core::open(cfg)?;
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
    ccm_core::http::serve(core, async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await
}
