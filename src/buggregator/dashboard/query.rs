//! The read side of the service dashboard: what the front-end shows. The
//! JSON shapes are those the front-end (`vue_components/service-*.html`) reads.

use super::{DAY, Instance};
use anyhow::Result;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use wikimisc::mysql_async::prelude::Queryable;
use wikimisc::mysql_async::{Conn, Row};

/// Seconds since the last push for "up" (pushes come every 60 s) …
const UP_WITHIN: i64 = 150;
/// … and for "stale"; older is "down".
const STALE_WITHIN: i64 = 600;
/// Longer ranges are read from the hourly rollup.
const RAW_MAX_RANGE: i64 = 2 * DAY;
const TARGET_POINTS: i64 = 360;
pub const RANGES: &[(&str, i64)] = &[("6h", 6 * 3600), ("24h", DAY), ("7d", 7 * DAY), ("30d", 30 * DAY)];
/// Ends that are no reason for an alert: planned ones, and a replacement
/// from outside without a final report (restart, deploy, pod moved).
const EXPECTED_END_REASONS: &[&str] = &["shutdown", "exit", "vanished"];
const MAX_EVENTS: u32 = 200;

/// A range name, `24h` if unknown.
pub fn range(name: &str) -> i64 {
    RANGES.iter().find(|(n, _)| *n == name).map_or(DAY, |(_, seconds)| *seconds)
}

pub fn status(instance: &Instance, now: i64) -> &'static str {
    if instance.ended_at.is_some() {
        return "stopped";
    }
    match now - instance.last_seen {
        age if age <= UP_WITHIN => "up",
        age if age <= STALE_WITHIN => "stale",
        _ => "down",
    }
}

/// All active services, with per-component status, restarts and uptime.
pub async fn overview(conn: &mut Conn, now: i64) -> Result<Value> {
    let services: Vec<(u64, String, String, String, Option<u64>)> =
        conn.query("SELECT id, name, label, url, tool_id FROM dash_service WHERE is_active=1 ORDER BY name").await?;
    let mut out = vec![];
    for (id, name, label, url, tool_id) in services {
        let components = components(conn, id, now).await?;
        out.push(json!({
            "label": if label.is_empty() { name.clone() } else { label },
            "name": name,
            "url": url,
            "tool_id": tool_id,
            "components": components,
        }));
    }
    Ok(Value::Array(out))
}

async fn components(conn: &mut Conn, service_id: u64, now: i64) -> Result<Value> {
    let rows: Vec<Row> = conn
        .exec(
            format!(
                "SELECT {} FROM dash_instance WHERE service_id=? AND (ended_at IS NULL OR ended_at>=?) ORDER BY id",
                Instance::COLUMNS
            ),
            (service_id, now - 7 * DAY),
        )
        .await?;
    let mut by_component: BTreeMap<String, Vec<Instance>> = BTreeMap::new();
    for row in &rows {
        let instance = Instance::from_row(row)?;
        by_component.entry(instance.component.clone()).or_default().push(instance);
    }
    let acks = acks(conn, service_id).await?;
    let mut out = Map::new();
    for (component, instances) in by_component {
        let running: Vec<&Instance> = instances.iter().filter(|i| status(i, now) == "up").collect();
        // The running replica closest to its memory limit (the newest when unknown), so the card shows the one at risk.
        let current = running
            .iter()
            .copied()
            .max_by(|a, b| a.memory_share().total_cmp(&b.memory_share()).then(a.id.cmp(&b.id)))
            .or(instances.last());
        let Some(current) = current else { continue };
        let deployed_at = deployed_at(conn, service_id, &component, &current.version).await?;
        out.insert(
            component.clone(),
            json!({
                "status": status(current, now),
                "replicas": running.len(),
                "current": instance_json(current),
                "deployed_at": deployed_at,
                "restarts_1h": ends_since(&instances, now - 3600),
                "restarts_24h": ends_since(&instances, now - DAY),
                "restarts_7d": ends_since(&instances, now - 7 * DAY),
                "uptime_24h": uptime(&instances, now - DAY, now),
                "uptime_7d": uptime(&instances, now - 7 * DAY, now),
                "last_unplanned_end": last_unplanned_end(&instances),
                "acks": acks.get(&component).cloned().unwrap_or_default(),
            }),
        );
    }
    Ok(Value::Object(out))
}

/// Start of the oldest instance running the current version without a different one in between.
async fn deployed_at(conn: &mut Conn, service_id: u64, component: &str, version: &str) -> Result<Option<i64>> {
    let before: Option<Option<u64>> = conn
        .exec_first(
            "SELECT MAX(id) FROM dash_instance WHERE service_id=? AND component=? AND version!=?",
            (service_id, component, version),
        )
        .await?;
    let start: Option<Option<i64>> = conn
        .exec_first(
            "SELECT MIN(started_at) FROM dash_instance WHERE service_id=? AND component=? AND id>?",
            (service_id, component, before.flatten().unwrap_or_default()),
        )
        .await?;
    Ok(start.flatten())
}

/// End reason => number of instances that ended since `since`.
fn ends_since(instances: &[Instance], since: i64) -> Map<String, Value> {
    let mut counts: BTreeMap<String, u64> = BTreeMap::new();
    for instance in instances.iter().filter(|i| i.ended_at.is_some_and(|e| e >= since)) {
        *counts.entry(instance.end_reason.clone().unwrap_or_default()).or_default() += 1;
    }
    counts.into_iter().map(|(reason, n)| (reason, n.into())).collect()
}

fn last_unplanned_end(instances: &[Instance]) -> Option<i64> {
    instances
        .iter()
        .filter(|i| !i.end_reason.as_deref().is_some_and(|r| EXPECTED_END_REASONS.contains(&r)))
        .filter_map(|i| i.ended_at)
        .max()
}

/// component => kind => acked_at
async fn acks(conn: &mut Conn, service_id: u64) -> Result<BTreeMap<String, Map<String, Value>>> {
    let rows: Vec<(String, String, i64)> =
        conn.exec("SELECT component, kind, acked_at FROM dash_ack WHERE service_id=?", (service_id,)).await?;
    let mut out: BTreeMap<String, Map<String, Value>> = BTreeMap::new();
    for (component, kind, acked_at) in rows {
        out.entry(component).or_default().insert(kind, acked_at.into());
    }
    Ok(out)
}

/// Share of `[from, to]` covered by at least one instance; a running, up
/// instance covers up to now. A service that started reporting within the
/// window is measured from then.
pub fn uptime(instances: &[Instance], from: i64, to: i64) -> f64 {
    let first = instances.iter().map(|i| i.started_at).min().unwrap_or(to);
    let from = from.max(first);
    if from >= to {
        return 1.0;
    }
    let mut intervals: Vec<(i64, i64)> = instances
        .iter()
        .filter_map(|i| {
            let end = i.ended_at.unwrap_or(if status(i, to) == "up" { to } else { i.last_seen });
            let (start, end) = (from.max(i.started_at), to.min(end));
            (end > start).then_some((start, end))
        })
        .collect();
    intervals.sort();
    let (mut covered, mut reach) = (0, from);
    for (start, end) in intervals {
        if end > reach {
            covered += end - start.max(reach);
            reach = end;
        }
    }
    (covered as f64 / (to - from) as f64 * 10_000.0).round() / 10_000.0
}

fn instance_json(i: &Instance) -> Value {
    json!({
        "uid": i.uid, "host": i.host, "version": i.version, "end_reason": i.end_reason, "end_detail": i.end_detail,
        "started_at": i.started_at, "last_seen": i.last_seen, "ended_at": i.ended_at,
        "rss_bytes": i.rss_bytes, "mem_limit_bytes": i.mem_limit_bytes,
    })
}

/// Series id, bucket start, counter sum, gauge average, last point in the bucket.
type BucketRow = (u64, i64, Option<f64>, Option<f64>, i64);

/// Time series of one component, downsampled to about `TARGET_POINTS`
/// buckets. Counters come as a rate per minute (0 where unchanged) summed
/// over all replicas; gauges as the bucket average of the highest replica.
pub async fn series(conn: &mut Conn, service: &str, component: &str, range: i64, now: i64) -> Result<Value> {
    let hourly = range > RAW_MAX_RANGE;
    let unit = if hourly { 3600 } else { 60 };
    let bucket = (range + TARGET_POINTS * unit - 1) / (TARGET_POINTS * unit) * unit;
    let bucket = bucket.max(unit);
    let from = (now - range) - (now - range).rem_euclid(bucket);
    let sql = if hourly {
        "SELECT p.series_id, p.hour - p.hour % ? AS b, SUM(p.delta_sum), AVG(p.value_avg), MAX(p.hour) + 3600
         FROM dash_point_hourly p JOIN dash_series s ON s.id=p.series_id JOIN dash_service v ON v.id=s.service_id
         WHERE v.name=? AND s.component=? AND p.hour>=? GROUP BY p.series_id, b ORDER BY b"
    } else {
        // Per instance first: with several replicas, a gauge shows the highest
        // replica's average (the one closest to its limit), not their mean.
        "SELECT series_id, b, SUM(d), MAX(a), MAX(last) FROM (
           SELECT p.series_id, p.instance_id, p.ts - p.ts % ? AS b, SUM(p.delta) AS d, AVG(p.value) AS a, MAX(p.ts) AS last
           FROM dash_point p JOIN dash_series s ON s.id=p.series_id JOIN dash_service v ON v.id=s.service_id
           WHERE v.name=? AND s.component=? AND p.ts>=? GROUP BY p.series_id, p.instance_id, b
         ) x GROUP BY series_id, b ORDER BY b"
    };
    let info: Vec<(u64, String, String, String)> = conn
        .exec(
            "SELECT s.id, s.name, s.labels, s.kind FROM dash_series s JOIN dash_service v ON v.id=s.service_id
             WHERE v.name=? AND s.component=? ORDER BY s.name, s.labels",
            (service, component),
        )
        .await?;
    let rows: Vec<BucketRow> = conn.exec(sql, (bucket, service, component, from)).await?;
    let mut points: BTreeMap<u64, Vec<(i64, f64)>> = BTreeMap::new();
    let kinds: BTreeMap<u64, &str> = info.iter().map(|(id, _, _, kind)| (*id, kind.as_str())).collect();
    for (series_id, b, delta, average, last) in rows {
        // A bucket's counts cover up to its last point; the current bucket is not complete yet.
        let span = bucket.min((last - b + unit).max(60));
        let value = match kinds.get(&series_id) {
            Some(&"counter") => delta.unwrap_or_default() * 60.0 / span as f64,
            _ => average.unwrap_or_default(),
        };
        points.entry(series_id).or_default().push((b, (value * 1e6).round() / 1e6));
    }
    let series: Vec<Value> = info
        .iter()
        .map(|(id, name, labels, kind)| {
            let mut points = points.remove(id).unwrap_or_default();
            if kind == "counter" {
                points = fill_zeros(&points, bucket, now);
            }
            let labels: Value = serde_json::from_str(labels).unwrap_or_else(|_| json!({}));
            json!({"name": name, "labels": labels, "kind": kind, "points": points.iter().map(|(t, v)| json!([t, v])).collect::<Vec<_>>()})
        })
        .collect();
    Ok(json!({"from": from, "to": now, "bucket": bucket, "series": series}))
}

/// Unchanged counters are not sent, so a bucket without a point after the
/// series' first one means no increase.
fn fill_zeros(points: &[(i64, f64)], bucket: i64, now: i64) -> Vec<(i64, f64)> {
    let Some(&(first, _)) = points.first() else { return vec![] };
    let values: BTreeMap<i64, f64> = points.iter().copied().collect();
    let mut out = vec![];
    let mut t = first;
    while t <= now {
        out.push((t, values.get(&t).copied().unwrap_or_default()));
        t += bucket;
    }
    out
}

/// Instances of one component seen within the range, newest first, each
/// flagged as a deploy when its version differs from its predecessor's.
pub async fn instances(conn: &mut Conn, service: &str, component: &str, range: i64, now: i64) -> Result<Value> {
    let columns = Instance::COLUMNS.split(", ").map(|c| format!("i.{c}")).collect::<Vec<_>>().join(", ");
    let rows: Vec<Row> = conn
        .exec(
            format!(
                "SELECT {columns} FROM dash_instance i JOIN dash_service v ON v.id=i.service_id
                 WHERE v.name=? AND i.component=? AND i.last_seen>=? ORDER BY i.id"
            ),
            (service, component, now - range),
        )
        .await?;
    let instances: Vec<Instance> = rows.iter().map(Instance::from_row).collect::<Result<_>>()?;
    let Some(first) = instances.first() else { return Ok(json!([])) };
    let mut previous_version: Option<String> = conn
        .exec_first(
            "SELECT version FROM dash_instance WHERE service_id=? AND component=? AND id<? ORDER BY id DESC LIMIT 1",
            (first.service_id, component, first.id),
        )
        .await?;
    let mut out = vec![];
    for instance in &instances {
        let mut o = instance_json(instance);
        o["deploy"] = json!(previous_version.as_ref().is_some_and(|v| *v != instance.version));
        o["status"] = json!(status(instance, now));
        previous_version = Some(instance.version.clone());
        out.push(o);
    }
    out.reverse();
    Ok(Value::Array(out))
}

pub async fn events(conn: &mut Conn, service: &str, range: i64, now: i64) -> Result<Value> {
    let rows: Vec<(i64, String, String, String, String)> = conn
        .exec(
            "SELECT e.ts, e.level, e.message, i.component, i.uid FROM dash_event e
             JOIN dash_service v ON v.id=e.service_id JOIN dash_instance i ON i.id=e.instance_id
             WHERE v.name=? AND e.ts>=? ORDER BY e.ts DESC, e.id DESC LIMIT ?",
            (service, now - range, MAX_EVENTS),
        )
        .await?;
    Ok(rows
        .into_iter()
        .map(|(ts, level, message, component, uid)| json!({"ts": ts, "level": level, "message": message, "component": component, "uid": uid}))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instance(id: u64, started_at: i64, last_seen: i64, ended_at: Option<i64>, reason: Option<&str>) -> Instance {
        Instance {
            id,
            service_id: 1,
            component: "web".into(),
            uid: format!("u{id}"),
            host: "h".into(),
            version: "1".into(),
            started_at,
            last_seen,
            ended_at,
            end_reason: reason.map(str::to_string),
            end_detail: String::new(),
            rss_bytes: None,
            mem_limit_bytes: None,
        }
    }

    #[test]
    fn statuses() {
        assert_eq!(status(&instance(1, 0, 1000, None, None), 1100), "up");
        assert_eq!(status(&instance(1, 0, 1000, None, None), 1500), "stale");
        assert_eq!(status(&instance(1, 0, 1000, None, None), 2000), "down");
        assert_eq!(status(&instance(1, 0, 1000, Some(1000), Some("exit")), 1001), "stopped");
    }

    #[test]
    fn uptime_with_a_gap() {
        // Covered 0-400 and 600-1000 of 0-1000; the second one is up at 1000.
        let instances = [instance(1, 0, 400, Some(400), Some("crash")), instance(2, 600, 990, None, None)];
        assert_eq!(uptime(&instances, 0, 1000), 0.8);
        // Measured from the first start.
        assert_eq!(uptime(&instances, -1000, 1000), 0.8);
        assert_eq!(uptime(&instances, 1000, 1000), 1.0);
    }

    #[test]
    fn restarts_and_unplanned_ends() {
        let instances = [
            instance(1, 0, 100, Some(100), Some("crash")),
            instance(2, 100, 200, Some(200), Some("shutdown")),
            instance(3, 200, 300, Some(300), Some("crash")),
        ];
        assert_eq!(Value::Object(ends_since(&instances, 150)), json!({"crash": 1, "shutdown": 1}));
        assert_eq!(last_unplanned_end(&instances), Some(300));
    }

    #[test]
    fn zeros_between_counter_points() {
        assert_eq!(fill_zeros(&[(0, 1.0), (120, 2.0)], 60, 200), vec![(0, 1.0), (60, 0.0), (120, 2.0), (180, 0.0)]);
        assert!(fill_zeros(&[], 60, 200).is_empty());
    }

    #[test]
    fn ranges() {
        assert_eq!(range("7d"), 7 * DAY);
        assert_eq!(range("nonsense"), DAY);
    }
}
