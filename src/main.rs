use anyhow::Result;
use clap::Parser;
use magnustools_rs::jobs::{self, Command};
use magnustools_rs::{config::Config, state::AppState, web};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use wikimisc::telemetry::Telemetry;

/// How often a stopped web server is restarted before the process gives up.
const MAX_SERVER_FAILURES: u8 = 5;

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
    let state = Arc::new(AppState::new(config)?);
    let router = web::router(state);
    let mut failures = 0u8;
    loop {
        let listener = match tokio::net::TcpListener::bind(("0.0.0.0", cli.port)).await {
            Ok(listener) => listener,
            Err(error) => {
                tracing::error!("cannot listen on port {}: {error}", cli.port);
                wikimisc::telemetry::shutdown(wikimisc::telemetry::ExitReason::Other(format!(
                    "cannot listen on port {}: {error}",
                    cli.port
                )))
                .await;
                return Err(error.into());
            }
        };
        match listener.local_addr() {
            Ok(address) => tracing::info!("listening on http://{address}"),
            Err(error) => tracing::warn!("cannot report the listen address: {error}"),
        }
        match wikimisc::server::serve(listener, router.clone()).await {
            Ok(()) => return Ok(()),
            Err(error) => {
                failures += 1;
                tracing::error!("web server stopped ({failures}): {error:#}");
                wikimisc::telemetry::event(wikimisc::telemetry::Level::Error, format!("web server stopped: {error}"));
                if failures >= MAX_SERVER_FAILURES {
                    wikimisc::telemetry::shutdown(wikimisc::telemetry::ExitReason::Other(format!(
                        "web server stopped: {error}"
                    )))
                    .await;
                    return Err(error.into());
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}
