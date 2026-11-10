//! The hourly dashboard maintenance: end inference, hourly rollups, daily
//! uses, retention, and re-inference after the inference changed.

use super::ingest::{STALE_AFTER, guess_end, infer_ends};
use super::{DAY, INFERRED_REASONS, Instance};
use anyhow::Result;
use wikimisc::mysql_async::Conn;
use wikimisc::mysql_async::Row;
use wikimisc::mysql_async::prelude::Queryable;

const RAW_DAYS: i64 = 7;
const HOURLY_DAYS: i64 = 400;
const EVENT_DAYS: i64 = 90;
const INSTANCE_DAYS: i64 = 400;

/// Meant to run hourly.
pub async fn maintenance(conn: &mut Conn, now: i64) -> Result<()> {
    // Catches instances whose successor stopped pushing too, so ingest never ran for them.
    let stale: Vec<(u64, String)> = conn
        .exec(
            "SELECT DISTINCT service_id, component FROM dash_instance WHERE ended_at IS NULL AND last_seen<?",
            (now - STALE_AFTER,),
        )
        .await?;
    for (service_id, component) in stale {
        infer_ends(conn, service_id, &component, now).await?;
    }
    rollup(conn, now).await?;
    super::daily_uses::copy(conn, now).await?;
    purge(conn, now).await
}

/// (Re)computes hourly rows from the last rolled-up hour up to the current,
/// incomplete one. Redoing the last hour picks up points that arrived late.
async fn rollup(conn: &mut Conn, now: i64) -> Result<()> {
    let last_hour: Option<Option<i64>> = conn.query_first("SELECT MAX(hour) FROM dash_point_hourly").await?;
    let from = match last_hour.flatten() {
        Some(hour) => hour,
        None => {
            let first: Option<Option<i64>> = conn.query_first("SELECT MIN(ts) FROM dash_point").await?;
            first.flatten().unwrap_or(now)
        }
    };
    let (from, to) = (from - from.rem_euclid(3600), now - now.rem_euclid(3600));
    // As in the series query: value_avg is the highest replica's average.
    conn.exec_drop(
        "INSERT INTO dash_point_hourly (series_id, hour, delta_sum, value_avg, value_max)
         SELECT series_id, h, SUM(d), MAX(a), MAX(m) FROM (
           SELECT series_id, instance_id, ts - ts % 3600 AS h, SUM(delta) AS d, AVG(value) AS a, MAX(value) AS m
           FROM dash_point WHERE ts>=? AND ts<? GROUP BY series_id, instance_id, h
         ) x GROUP BY series_id, h
         ON DUPLICATE KEY UPDATE delta_sum=VALUES(delta_sum), value_avg=VALUES(value_avg), value_max=VALUES(value_max)",
        (from, to),
    )
    .await?;
    Ok(())
}

async fn purge(conn: &mut Conn, now: i64) -> Result<()> {
    conn.exec_drop("DELETE FROM dash_point WHERE ts<?", (now - RAW_DAYS * DAY,)).await?;
    conn.exec_drop("DELETE FROM dash_point_hourly WHERE hour<?", (now - HOURLY_DAYS * DAY,)).await?;
    conn.exec_drop("DELETE FROM dash_event WHERE ts<?", (now - EVENT_DAYS * DAY,)).await?;
    // Ended instances send no more counters.
    conn.exec_drop(
        "DELETE dash_counter_state FROM dash_counter_state JOIN dash_instance ON dash_instance.id=instance_id
         WHERE ended_at<?",
        (now - DAY,),
    )
    .await?;
    // Keep instance history as long as the hourly rollup; points/events above
    // are already gone, and their foreign keys cascade for anything left.
    conn.exec_drop("DELETE FROM dash_instance WHERE ended_at<?", (now - INSTANCE_DAYS * DAY,)).await?;
    Ok(())
}

/// Re-classifies instances whose end was inferred, e.g. after the inference
/// improved. Returns `(uid, old reason, new reason)` for the changed ones.
pub async fn reinfer_ends(conn: &mut Conn) -> Result<Vec<(String, String, String)>> {
    let reasons = INFERRED_REASONS.iter().map(|r| format!("'{r}'")).collect::<Vec<_>>().join(",");
    let rows: Vec<Row> =
        conn.query(format!("SELECT {} FROM dash_instance WHERE end_reason IN ({reasons})", Instance::COLUMNS)).await?;
    let mut changed = vec![];
    for row in &rows {
        let old = Instance::from_row(row)?;
        let Some((reason, detail)) = guess_end(conn, &old).await? else { continue };
        let old_reason = old.end_reason.clone().unwrap_or_default();
        if reason == old_reason && detail == old.end_detail {
            continue;
        }
        conn.exec_drop("UPDATE dash_instance SET end_reason=?, end_detail=? WHERE id=?", (reason, detail, old.id))
            .await?;
        if reason != old_reason {
            changed.push((old.uid, old_reason, reason.to_string()));
        }
    }
    Ok(changed)
}
