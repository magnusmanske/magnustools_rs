use anyhow::Result;
use clap::Parser;
use magnustools_rs::jobs::{self, Command};
use magnustools_rs::{config::Config, state::AppState, web};
use std::path::PathBuf;
use std::sync::Arc;
use wikimisc::telemetry::Telemetry;

/// The magnustools web service; or, with a command, one of its jobs.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Cli {
    /// Path to the JSON configuration file.
    #[arg(long, default_value = "config.json")]
    config: PathBuf,
    /// Port to listen on. Toolforge sets `PORT`.
    #[arg(long, env = "PORT", default_value_t = 8000)]
    port: u16,
    #[command(subcommand)]
    command: Option<Command>,
}

#[tokio::main]
async fn main() -> Result<()> {
    wikimisc::logging::init();
    let cli = Cli::parse();
    let config = Config::load(&cli.config)?;
    if let Some(command) = cli.command {
        let state = AppState::new(config)?;
        return jobs::run(command, &state).await;
    }
    // `server::serve` drains requests on SIGTERM, then sends the final push.
    Telemetry::builder("web")
        .config(&config.telemetry)
        .version(env!("CARGO_PKG_VERSION"))
        .handle_signals(false)
        .install_or_log();
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", cli.port)).await?;
    tracing::info!("listening on http://{}", listener.local_addr()?);
    let state = AppState::new(config)?;
    wikimisc::server::serve(listener, web::router(Arc::new(state))).await?;
    Ok(())
}
