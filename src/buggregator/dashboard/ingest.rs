//! Storing pushes: authenticating the token, tracking process instances,
//! recording metric points and events, and working out how instances that
//! went away without saying why most likely ended.

use super::payload::Push;
use super::{INFERRED_REASONS, Instance};
use anyhow::Result;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use wikimisc::mysql_async::prelude::Queryable;
use wikimisc::mysql_async::{Conn, Params, Row, Value};
use wikimisc::telemetry_wire::{MIN_PUSH_GAP_SECS, MetricKind};

/// An instance silent this long, with a successor, has ended.
pub const STALE_AFTER: i64 = 180;
/// A last memory reading at or above this share of the limit means a likely OOM kill.
const OOM_FRACTION: f64 = 0.9;
/// Per service and component, to bound label explosions.
const MAX_SERIES: usize = 500;
const RSS_METRIC: &str = "process_resident_memory_bytes";
const LIMIT_METRIC: &str = "process_memory_limit_bytes";

/// A refused push: HTTP status and message.
#[derive(Debug, PartialEq, Eq)]
pub struct Rejected(pub u16, pub &'static str);

pub fn hash_token(token: &str) -> String {
    Sha256::digest(token.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// The active service a token belongs to. Tokens are 256 random bits, so
/// looking up their hash needs no constant-time comparison.
pub async fn service_for_token(conn: &mut Conn, token: &str) -> Result<Option<u64>> {
    if token.len() != 64 || !token.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)) {
        return Ok(None);
    }
    let id =
        conn.exec_first("SELECT id FROM dash_service WHERE token_hash=? AND is_active=1", (hash_token(token),)).await?;
    Ok(id)
}

/// Stores a push; the number of metrics and events skipped, or why it was refused.
pub async fn ingest(conn: &mut Conn, service_id: u64, push: &Push, now: i64) -> Result<Result<usize, Rejected>> {
    let row: Option<Row> = conn
        .exec_first(
            format!("SELECT {} FROM dash_instance WHERE service_id=? AND uid=?", Instance::COLUMNS),
            (service_id, &push.uid),
        )
        .await?;
    let instance_id = match row.as_ref().map(Instance::from_row).transpose()? {
        None => create_instance(conn, service_id, push, now).await?,
        Some(instance) => {
            if let Err(rejected) = check_can_push(conn, &instance, push, now).await? {
                return Ok(Err(rejected));
            }
            instance.id
        }
    };
    let skipped = push.skipped + store_points(conn, service_id, push, instance_id, now).await?;
    for event in &push.events {
        conn.exec_drop(
            "INSERT INTO dash_event (service_id, instance_id, ts, level, message) VALUES (?, ?, ?, ?, ?)",
            (service_id, instance_id, event.ts, event.level, &event.message),
        )
        .await?;
    }
    update_instance(conn, instance_id, push, now).await?;
    infer_ends(conn, service_id, &push.component, now).await?;
    Ok(Ok(skipped))
}

async fn create_instance(conn: &mut Conn, service_id: u64, push: &Push, now: i64) -> Result<u64> {
    conn.exec_drop(
        "INSERT INTO dash_instance (service_id, component, uid, host, version, started_at, last_seen)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
        (service_id, &push.component, &push.uid, &push.host, &push.version, push.started, now),
    )
    .await?;
    conn.last_insert_id().ok_or_else(|| anyhow::anyhow!("no dash_instance id"))
}

async fn check_can_push(conn: &mut Conn, instance: &Instance, push: &Push, now: i64) -> Result<Result<(), Rejected>> {
    if instance.ended_at.is_some() {
        // An instance wrongly declared dead (e.g. after a network outage) comes
        // back to life; one that reported its own end does not.
        let inferred = instance.end_reason.as_deref().is_some_and(|r| INFERRED_REASONS.contains(&r));
        if !inferred {
            return Ok(Err(Rejected(409, "Instance has already ended")));
        }
        conn.exec_drop(
            "UPDATE dash_instance SET ended_at=NULL, end_reason=NULL, end_detail='' WHERE id=?",
            (instance.id,),
        )
        .await?;
    }
    let gap = i64::try_from(MIN_PUSH_GAP_SECS).unwrap_or(15);
    if push.exit.is_none() && instance.last_seen > now - gap {
        return Ok(Err(Rejected(429, "Too many pushes")));
    }
    Ok(Ok(()))
}

async fn update_instance(conn: &mut Conn, instance_id: u64, push: &Push, now: i64) -> Result<()> {
    conn.exec_drop(
        "UPDATE dash_instance SET last_seen=?, version=?, rss_bytes=COALESCE(?, rss_bytes),
         mem_limit_bytes=COALESCE(?, mem_limit_bytes) WHERE id=?",
        (now, &push.version, push.metric_value(RSS_METRIC), push.metric_value(LIMIT_METRIC), instance_id),
    )
    .await?;
    if let Some(exit) = &push.exit {
        conn.exec_drop(
            "UPDATE dash_instance SET ended_at=?, end_reason=?, end_detail=? WHERE id=?",
            (now, exit.reason, &exit.detail, instance_id),
        )
        .await?;
    }
    Ok(())
}

fn kind_name(kind: MetricKind) -> &'static str {
    match kind {
        MetricKind::Counter => "counter",
        MetricKind::Gauge => "gauge",
    }
}

/// Returns the number of metrics skipped.
async fn store_points(conn: &mut Conn, service_id: u64, push: &Push, instance_id: u64, now: i64) -> Result<usize> {
    // "name\nlabels" => (series id, kind)
    let rows: Vec<(u64, String, String, String)> = conn
        .exec(
            "SELECT id, name, labels, kind FROM dash_series WHERE service_id=? AND component=?",
            (service_id, &push.component),
        )
        .await?;
    let mut series: HashMap<String, (u64, String)> =
        rows.into_iter().map(|(id, name, labels, kind)| (format!("{name}\n{labels}"), (id, kind))).collect();
    let previous: HashMap<u64, f64> = conn
        .exec("SELECT series_id, value FROM dash_counter_state WHERE instance_id=?", (instance_id,))
        .await?
        .into_iter()
        .collect();

    let mut skipped = 0;
    // (series id, value, delta for counters)
    let mut points: Vec<(u64, f64, Option<f64>)> = vec![];
    for metric in &push.metrics {
        let key = format!("{}\n{}", metric.name, metric.labels);
        let kind = kind_name(metric.kind);
        let (series_id, series_kind) = match series.get(&key) {
            Some(known) => known.clone(),
            None if series.len() >= MAX_SERIES => {
                skipped += 1;
                continue;
            }
            None => {
                conn.exec_drop(
                    "INSERT INTO dash_series (service_id, component, name, labels, kind) VALUES (?, ?, ?, ?, ?)",
                    (service_id, &push.component, &metric.name, &metric.labels, kind),
                )
                .await?;
                let id = conn.last_insert_id().ok_or_else(|| anyhow::anyhow!("no dash_series id"))?;
                series.insert(key, (id, kind.to_string()));
                (id, kind.to_string())
            }
        };
        if series_kind != kind {
            skipped += 1;
            continue;
        }
        let delta =
            (metric.kind == MetricKind::Counter).then(|| delta(metric.value, previous.get(&series_id).copied()));
        points.push((series_id, metric.value, delta));
    }

    if !points.is_empty() {
        let sql = format!(
            "INSERT IGNORE INTO dash_point (series_id, instance_id, ts, value, delta) VALUES {}",
            vec!["(?, ?, ?, ?, ?)"; points.len()].join(",")
        );
        let params: Vec<Value> = points
            .iter()
            .flat_map(|(series_id, value, delta)| {
                [(*series_id).into(), instance_id.into(), now.into(), (*value).into(), (*delta).into()]
            })
            .collect();
        conn.exec_drop(sql, Params::Positional(params)).await?;
    }
    let counters: Vec<_> = points.iter().filter(|(.., delta)| delta.is_some()).collect();
    if !counters.is_empty() {
        let sql = format!(
            "INSERT INTO dash_counter_state (instance_id, series_id, value) VALUES {}
             ON DUPLICATE KEY UPDATE value=VALUES(value)",
            vec!["(?, ?, ?)"; counters.len()].join(",")
        );
        let params: Vec<Value> = counters
            .iter()
            .flat_map(|(series_id, value, _)| [instance_id.into(), (*series_id).into(), (*value).into()])
            .collect();
        conn.exec_drop(sql, Params::Positional(params)).await?;
    }
    Ok(skipped)
}

/// Counters are totals since process start; a lower total than last time
/// means the counter was reset, so the whole value is new.
fn delta(value: f64, previous: Option<f64>) -> f64 {
    match previous {
        Some(previous) if value >= previous => value - previous,
        _ => value,
    }
}

/// Instances that stopped pushing without a final push, and have since been
/// replaced by a newer instance, have ended; guess how. One that was not
/// replaced is simply down, which the dashboard shows from `last_seen`.
pub async fn infer_ends(conn: &mut Conn, service_id: u64, component: &str, now: i64) -> Result<()> {
    let rows: Vec<Row> = conn
        .exec(
            format!(
                "SELECT {} FROM dash_instance WHERE service_id=? AND component=? AND ended_at IS NULL AND last_seen<? ORDER BY id",
                Instance::COLUMNS
            ),
            (service_id, component, now - STALE_AFTER),
        )
        .await?;
    for row in &rows {
        let old = Instance::from_row(row)?;
        if let Some((reason, detail)) = guess_end(conn, &old).await? {
            conn.exec_drop(
                "UPDATE dash_instance SET ended_at=last_seen, end_reason=?, end_detail=? WHERE id=? AND ended_at IS NULL",
                (reason, detail, old.id),
            )
            .await?;
        }
    }
    Ok(())
}

/// How an instance that stopped pushing ended, or `None` while no newer
/// instance of its component exists (then it is just down).
pub async fn guess_end(conn: &mut Conn, old: &Instance) -> Result<Option<(&'static str, String)>> {
    // With several replicas, a newer instance on the same host (pod) is the one that replaced it.
    let next: Option<(String, String)> = conn
        .exec_first(
            "SELECT host, version FROM dash_instance WHERE service_id=? AND component=? AND id>?
             ORDER BY host=? DESC, id LIMIT 1",
            (old.service_id, &old.component, old.id, &old.host),
        )
        .await?;
    let Some((next_host, next_version)) = next else { return Ok(None) };
    let previous_rss: Option<f64> = conn
        .exec_first(
            "SELECT p.value FROM dash_point p JOIN dash_series s ON s.id=p.series_id
             WHERE p.instance_id=? AND s.name=? AND s.labels='{}' ORDER BY p.ts DESC LIMIT 1 OFFSET 1",
            (old.id, RSS_METRIC),
        )
        .await?;
    let end = End {
        rss: old.rss_bytes.unwrap_or_default(),
        limit: old.mem_limit_bytes.unwrap_or_default(),
        previous_rss: previous_rss.map(|v| v as u64),
        old_host: &old.host,
        new_host: &next_host,
        old_version: &old.version,
        new_version: &next_version,
    };
    Ok(Some(end.reason()))
}

/// What is known about an instance that ended without a final push.
struct End<'a> {
    rss: u64,
    limit: u64,
    previous_rss: Option<u64>,
    old_host: &'a str,
    new_host: &'a str,
    old_version: &'a str,
    new_version: &'a str,
}

impl End<'_> {
    /// Memory is only read once a minute, so an OOM kill usually happens
    /// after the last reading. Besides a last reading near the limit, a sharp
    /// rise just before the end counts as likely OOM: the process may have
    /// lived for up to another interval, so the rise is projected twice that far.
    ///
    /// Otherwise: a new version took over means a deploy. On Kubernetes, a
    /// container restarted in place keeps its pod's host name (a crash),
    /// whereas a restart or deploy creates a new pod; such a replacement
    /// ('vanished') is not an alert, as it comes from outside.
    fn reason(&self) -> (&'static str, String) {
        let mib = |bytes: u64| (bytes as f64 / 1_048_576.0).round();
        if self.rss > 0 && self.limit > 0 {
            let (rss, limit) = (self.rss as f64, self.limit as f64);
            if rss >= OOM_FRACTION * limit {
                let percent = (100.0 * rss / limit).round();
                return ("oom", format!("Likely out of memory: last reading was {percent}% of the limit"));
            }
            if let Some(previous) = self.previous_rss.filter(|p| *p < self.rss) {
                let rise = (self.rss - previous) as f64;
                if rss + 2.0 * rise >= OOM_FRACTION * limit {
                    return (
                        "oom",
                        format!(
                            "Likely out of memory: memory rose from {} to {} MiB in the last minute (limit {} MiB)",
                            mib(previous),
                            mib(self.rss),
                            mib(self.limit)
                        ),
                    );
                }
            }
        }
        if !self.old_version.is_empty() && !self.new_version.is_empty() && self.old_version != self.new_version {
            return ("vanished", format!("Replaced by a new version ({}) without a final report", self.new_version));
        }
        if !self.old_host.is_empty() && self.old_host == self.new_host {
            return ("crash", "Restarted on the same host without reporting a reason".to_string());
        }
        ("vanished", "Replaced by a new pod without a final report (restart, or the pod was moved)".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn end<'a>(
        rss: u64,
        limit: u64,
        previous: Option<u64>,
        hosts: (&'a str, &'a str),
        versions: (&'a str, &'a str),
    ) -> End<'a> {
        End {
            rss,
            limit,
            previous_rss: previous,
            old_host: hosts.0,
            new_host: hosts.1,
            old_version: versions.0,
            new_version: versions.1,
        }
    }

    const MIB: u64 = 1_048_576;

    #[test]
    fn end_reasons() {
        let (reason, detail) = end(95 * MIB, 100 * MIB, None, ("a", "b"), ("1", "1")).reason();
        assert_eq!((reason, detail.as_str()), ("oom", "Likely out of memory: last reading was 95% of the limit"));
        let (reason, detail) = end(60 * MIB, 100 * MIB, Some(45 * MIB), ("a", "b"), ("1", "1")).reason();
        assert_eq!(reason, "oom");
        assert_eq!(detail, "Likely out of memory: memory rose from 45 to 60 MiB in the last minute (limit 100 MiB)");
        assert_eq!(end(10 * MIB, 100 * MIB, Some(9 * MIB), ("a", "b"), ("1", "2")).reason().0, "vanished");
        assert_eq!(end(0, 0, None, ("pod", "pod"), ("1", "1")).reason().0, "crash");
        assert_eq!(end(0, 0, None, ("pod-a", "pod-b"), ("1", "1")).reason().0, "vanished");
    }

    #[test]
    fn deltas() {
        assert_eq!(delta(10.0, None), 10.0);
        assert_eq!(delta(10.0, Some(4.0)), 6.0);
        assert_eq!(delta(3.0, Some(4.0)), 3.0);
    }

    #[test]
    fn token_hashes() {
        assert_eq!(hash_token("abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }
}
