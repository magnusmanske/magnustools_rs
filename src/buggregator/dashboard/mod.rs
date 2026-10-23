//! The service dashboard: Rust services push their health once a minute
//! (`ingest`); the buggregator front-end shows it (`query`); `admin`
//! registers services and keeps the tables in shape; `alerts` finds what is
//! offline, for the `dashboard-alerts` job.

pub mod admin;
pub mod alerts;
pub mod ingest;
pub mod payload;
pub mod query;

use anyhow::{Result, anyhow};
use std::time::{SystemTime, UNIX_EPOCH};
use wikimisc::mysql_async::Row;
use wikimisc::mysql_async::prelude::FromValue;

pub const DAY: i64 = 86_400;

/// Ends the dashboard worked out itself, as opposed to ones a service reported.
pub const INFERRED_REASONS: &[&str] = &["oom", "crash", "vanished"];

pub fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs().try_into().unwrap_or(i64::MAX))
}

/// A process of a service, as `dash_instance` has it.
#[derive(Debug, Clone, PartialEq)]
pub struct Instance {
    pub id: u64,
    pub service_id: u64,
    pub component: String,
    pub uid: String,
    pub host: String,
    pub version: String,
    pub started_at: i64,
    pub last_seen: i64,
    pub ended_at: Option<i64>,
    pub end_reason: Option<String>,
    pub end_detail: String,
    pub rss_bytes: Option<u64>,
    pub mem_limit_bytes: Option<u64>,
}

impl Instance {
    pub const COLUMNS: &'static str = "id, service_id, component, uid, host, version, started_at, last_seen, \
        ended_at, end_reason, end_detail, rss_bytes, mem_limit_bytes";

    /// A row with [`Self::COLUMNS`].
    pub fn from_row(row: &Row) -> Result<Self> {
        Ok(Self {
            id: col(row, 0)?,
            service_id: col(row, 1)?,
            component: col(row, 2)?,
            uid: col(row, 3)?,
            host: col(row, 4)?,
            version: col(row, 5)?,
            started_at: col(row, 6)?,
            last_seen: col(row, 7)?,
            ended_at: col(row, 8)?,
            end_reason: col(row, 9)?,
            end_detail: col(row, 10)?,
            rss_bytes: col(row, 11)?,
            mem_limit_bytes: col(row, 12)?,
        })
    }

    /// Share of the memory limit in use, 0 if unknown.
    pub fn memory_share(&self) -> f64 {
        match (self.rss_bytes, self.mem_limit_bytes) {
            (Some(rss), Some(limit)) if limit > 0 => rss as f64 / limit as f64,
            _ => 0.0,
        }
    }
}

/// Column `index` of a row.
pub fn col<T: FromValue>(row: &Row, index: usize) -> Result<T> {
    row.get_opt(index).ok_or_else(|| anyhow!("no column {index}"))?.map_err(|e| anyhow!("column {index}: {e}"))
}
