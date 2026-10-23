//! Background jobs and admin commands: `magnustools_rs <command>`. On
//! Toolforge, the jobs framework runs them (see `jobs.yaml`).

mod monitor;

use crate::buggregator::dashboard::{self, admin};
use crate::buggregator::{issues, update};
use crate::state::AppState;
use anyhow::{Result, bail};
use clap::Subcommand;
use wikimisc::mysql_async::prelude::Queryable;

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Issues from talk pages, GitHub and Codeberg; Toolhub data; tidying up (daily).
    BuggregatorUpdate,
    /// Only the tidying up of issues: dates, tools, closing old ones.
    BuggregatorMaintenance,
    /// Tool descriptions and keywords from Toolhub.
    Toolhub,
    /// What each tool's web address answers (twice an hour).
    Webstatus,
    /// Service dashboard: end inference, rollups, retention (hourly).
    DashboardMaintenance,
    /// Checks tools, writes tool_status.html, mails new alerts (twice an hour).
    Monitor,
    /// Service dashboard administration.
    #[command(subcommand)]
    Dashboard(DashboardCommand),
    /// Sets the status of an issue (OPEN or CLOSED).
    IssueStatus { issue_id: u64, status: String },
}

#[derive(Debug, Subcommand)]
pub enum DashboardCommand {
    /// Registers a service and prints its token, which cannot be shown again.
    Add {
        name: String,
        label: Option<String>,
        url: Option<String>,
        tool_id: Option<u64>,
    },
    /// Prints a new token; the old one stops working at once.
    Rotate {
        name: String,
    },
    Enable {
        name: String,
    },
    Disable {
        name: String,
    },
    List,
    /// Re-classifies inferred ends, after the inference changed.
    Reinfer,
}

pub async fn run(command: Command, state: &AppState) -> Result<()> {
    let mut conn = state.tool_db.conn().await?;
    let now = dashboard::now();
    match command {
        Command::BuggregatorUpdate => update::update(state).await?,
        Command::BuggregatorMaintenance => update::maintenance(&mut conn).await?,
        Command::Toolhub => update::toolhub(state, &mut conn).await?,
        Command::Webstatus => update::web_status(&mut conn).await?,
        Command::DashboardMaintenance => admin::maintenance(&mut conn, now).await?,
        Command::Monitor => monitor::run(state).await?,
        Command::IssueStatus { issue_id, status } => {
            let status = status.to_uppercase();
            if !issues::STATUSES.contains(&status.as_str()) {
                bail!("'{status}' is not a valid issue status");
            }
            conn.exec_drop("UPDATE issue SET status=?, date_last=CURRENT_TIMESTAMP WHERE id=?", (status, issue_id))
                .await?;
        }
        Command::Dashboard(command) => match command {
            DashboardCommand::Add { name, label, url, tool_id } => {
                let label = label.unwrap_or_default();
                println!(
                    "{}",
                    admin::add_service(&mut conn, &name, &label, &url.unwrap_or_default(), tool_id, now).await?
                );
            }
            DashboardCommand::Rotate { name } => println!("{}", admin::rotate_token(&mut conn, &name).await?),
            DashboardCommand::Enable { name } => admin::set_active(&mut conn, &name, true).await?,
            DashboardCommand::Disable { name } => admin::set_active(&mut conn, &name, false).await?,
            DashboardCommand::List => {
                for (name, active, tool_id, label) in admin::list_services(&mut conn).await? {
                    let tool_id = tool_id.map_or_else(|| "-".to_string(), |id| id.to_string());
                    println!("{name:<24} {:<8} {tool_id:<8} {label}", if active { "active" } else { "inactive" });
                }
            }
            DashboardCommand::Reinfer => {
                for (uid, from, to) in admin::reinfer_ends(&mut conn).await? {
                    println!("{uid}: {from} => {to}");
                }
            }
        },
    }
    Ok(())
}
