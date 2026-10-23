//! The tool's own database on ToolsDB (`<user>__buggregator`): tool list,
//! usage log, issues and service dashboard.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;
use wikimisc::mysql_async::Conn;
use wikimisc::toolforge::pool::{CustomPool, ToolsDbPool};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolDbConfig {
    /// `{user}` stands for the tool's database user.
    #[serde(default = "default_database")]
    pub database: String,
    /// For development: a `mysql://` URL to use instead of ToolsDB.
    #[serde(default)]
    pub url: Option<String>,
}

impl Default for ToolDbConfig {
    fn default() -> Self {
        Self { database: default_database(), url: None }
    }
}

fn default_database() -> String {
    "{user}__buggregator".to_string()
}

pub enum ToolDb {
    ToolsDb { pool: ToolsDbPool, database: String },
    Url(CustomPool),
}

impl std::fmt::Debug for ToolDb {
    // Not derived: the pools must not print their connection options.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ToolsDb { database, .. } => write!(f, "ToolDb({database})"),
            Self::Url(pool) => write!(f, "ToolDb({})", pool.server()),
        }
    }
}

impl ToolDb {
    pub fn new(config: &ToolDbConfig, credentials: &Path) -> Result<Self> {
        if let Some(url) = &config.url {
            let pool = CustomPool::builder_from_url(url).context("invalid tool_db.url")?.build()?;
            return Ok(Self::Url(pool));
        }
        let pool = ToolsDbPool::builder()
            .my_cnf_path(credentials)
            .build()
            .with_context(|| format!("cannot read the database credentials from {}", credentials.display()))?;
        let database = config.database.replace("{user}", pool.user());
        Ok(Self::ToolsDb { pool, database })
    }

    pub async fn conn(&self) -> Result<Conn> {
        let conn = match self {
            Self::ToolsDb { pool, database } => pool.connect(database).await,
            Self::Url(pool) => pool.connect().await,
        };
        conn.context("cannot connect to the tool database")
    }
}
