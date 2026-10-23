//! Service registration, token handling, alert acknowledgements, and the
//! hourly maintenance (end inference, rollups, retention).

use super::ingest::{STALE_AFTER, guess_end, hash_token, infer_ends};
use super::{DAY, INFERRED_REASONS, Instance};
use anyhow::{Result, bail};
use wikimisc::mysql_async::Conn;
use wikimisc::mysql_async::Row;
use wikimisc::mysql_async::prelude::Queryable;

const RAW_DAYS: i64 = 7;
const HOURLY_DAYS: i64 = 400;
const EVENT_DAYS: i64 = 90;
pub const ACK_KINDS: &[&str] = &["status", "restarts", "memory"];

/// Hides a dashboard alert until whatever triggers it happens again. The
/// error is for the user.
pub async fn acknowledge(
    conn: &mut Conn,
    service: &str,
    component: &str,
    kind: &str,
    user: &str,
    now: i64,
) -> Result<Result<(), String>> {
    if !ACK_KINDS.contains(&kind) {
        return Ok(Err(format!("Invalid alert kind '{kind}'")));
    }
    let service_id: Option<u64> = conn
        .exec_first(
            "SELECT DISTINCT service_id FROM dash_instance
             WHERE service_id=(SELECT id FROM dash_service WHERE name=?) AND component=?",
            (service, component),
        )
        .await?;
    let Some(service_id) = service_id else {
        return Ok(Err(format!("No component '{component}' of service '{service}'")));
    };
    conn.exec_drop(
        "INSERT INTO dash_ack (service_id, component, kind, acked_at, acked_by) VALUES (?, ?, ?, ?, ?)
         ON DUPLICATE KEY UPDATE acked_at=VALUES(acked_at), acked_by=VALUES(acked_by)",
        (service_id, component, kind, now, user),
    )
    .await?;
    Ok(Ok(()))
}

/// Registers a service; returns its token, which is not stored and cannot be shown again.
pub async fn add_service(
    conn: &mut Conn,
    name: &str,
    label: &str,
    url: &str,
    tool_id: Option<u64>,
    now: i64,
) -> Result<String> {
    let valid = name.len() <= 64
        && name.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
    if !valid {
        bail!("Invalid service name '{name}'");
    }
    let token = new_token();
    conn.exec_drop(
        "INSERT INTO dash_service (name, label, url, tool_id, token_hash, created_at) VALUES (?, ?, ?, ?, ?, ?)",
        (name, label, url, tool_id, hash_token(&token), now),
    )
    .await?;
    Ok(token)
}

/// Replaces a service's token; the old one stops working at once.
pub async fn rotate_token(conn: &mut Conn, name: &str) -> Result<String> {
    let token = new_token();
    conn.exec_drop("UPDATE dash_service SET token_hash=? WHERE name=?", (hash_token(&token), name)).await?;
    if conn.affected_rows() == 0 {
        bail!("No service '{name}'");
    }
    Ok(token)
}

pub async fn set_active(conn: &mut Conn, name: &str, active: bool) -> Result<()> {
    conn.exec_drop("UPDATE dash_service SET is_active=? WHERE name=?", (active, name)).await?;
    Ok(())
}

/// `(name, active, tool id, label)` of all services.
pub async fn list_services(conn: &mut Conn) -> Result<Vec<(String, bool, Option<u64>, String)>> {
    Ok(conn.query("SELECT name, is_active, tool_id, label FROM dash_service ORDER BY name").await?)
}

fn new_token() -> String {
    rand::random::<[u8; 32]>().iter().map(|b| format!("{b:02x}")).collect()
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens() {
        let token = new_token();
        assert_eq!(token.len(), 64);
        assert!(token.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_ne!(token, new_token());
    }
}
