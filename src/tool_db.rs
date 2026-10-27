//! The tool's own database on ToolsDB (`<user>__buggregator`): tool list,
//! usage log, issues and service dashboard.

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};
use wikimisc::mysql_async::Conn;
use wikimisc::toolforge::pool::{CustomPool, ToolsDbPool};

/// Getting a connection takes milliseconds; this long means none is coming.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Connections timing out for this long, none getting through, means the pool
/// is stuck (e.g. on connections that died unnoticed); only a restart helps.
const STUCK_AFTER: Duration = Duration::from_secs(60);

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

enum Pool {
    ToolsDb { pool: ToolsDbPool, database: String },
    Url(CustomPool),
}

pub struct ToolDb {
    pool: Pool,
    timeouts: Timeouts,
}

impl std::fmt::Debug for ToolDb {
    // Not derived: the pools must not print their connection options.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.pool {
            Pool::ToolsDb { database, .. } => write!(f, "ToolDb({database})"),
            Pool::Url(pool) => write!(f, "ToolDb({})", pool.server()),
        }
    }
}

impl ToolDb {
    pub fn new(config: &ToolDbConfig, credentials: &Path) -> Result<Self> {
        let pool = match &config.url {
            Some(url) => Pool::Url(CustomPool::builder_from_url(url).context("invalid tool_db.url")?.build()?),
            None => {
                let pool = ToolsDbPool::builder()
                    .my_cnf_path(credentials)
                    .build()
                    .with_context(|| format!("cannot read the database credentials from {}", credentials.display()))?;
                let database = config.database.replace("{user}", pool.user());
                Pool::ToolsDb { pool, database }
            }
        };
        Ok(Self { pool, timeouts: Timeouts::default() })
    }

    pub async fn conn(&self) -> Result<Conn> {
        let connect = async {
            match &self.pool {
                Pool::ToolsDb { pool, database } => pool.connect(database).await,
                Pool::Url(pool) => pool.connect().await,
            }
        };
        let result = tokio::time::timeout(CONNECT_TIMEOUT, connect).await;
        self.timeouts.record(result.is_err(), Instant::now());
        match result {
            Ok(conn) => conn.context("cannot connect to the tool database"),
            Err(_) => bail!("no connection to the tool database within {CONNECT_TIMEOUT:?}"),
        }
    }

    /// Whether connections have only been timing out for a while; the health
    /// check then fails, so the pod is restarted.
    pub fn is_stuck(&self) -> bool {
        self.timeouts.is_stuck(Instant::now())
    }
}

/// Since when getting connections has only been timing out.
#[derive(Debug, Default)]
struct Timeouts(Mutex<Option<Instant>>);

impl Timeouts {
    fn record(&self, timed_out: bool, now: Instant) {
        let mut since = self.since();
        if timed_out {
            since.get_or_insert(now);
        } else {
            *since = None;
        }
    }

    fn is_stuck(&self, now: Instant) -> bool {
        self.since().is_some_and(|since| now.duration_since(since) >= STUCK_AFTER)
    }

    fn since(&self) -> MutexGuard<'_, Option<Instant>> {
        // Only ever holds an `Option<Instant>`, which a panic cannot leave half-written.
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stuck_only_after_timing_out_for_a_while() {
        let (timeouts, start) = (Timeouts::default(), Instant::now());
        let later = |seconds| start + Duration::from_secs(seconds);
        assert!(!timeouts.is_stuck(later(600)));
        timeouts.record(true, start);
        timeouts.record(true, later(30));
        assert!(!timeouts.is_stuck(later(59)));
        assert!(timeouts.is_stuck(later(60)));
        timeouts.record(false, later(61));
        assert!(!timeouts.is_stuck(later(600)));
    }
}
