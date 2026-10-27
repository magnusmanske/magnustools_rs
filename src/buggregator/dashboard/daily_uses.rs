//! Copies the uses that the Rust services count in their telemetry
//! (`tool_uses_total{tool}`) into the logger's tables, one row per tool and
//! day: as tool `<service>`, method `<tool>`. So their history is as long,
//! and in the same place, as that of the tools that call the logger.

use super::DAY;
use crate::tools::logger::{day_number, set_uses};
use anyhow::Result;
use chrono::{DateTime, Days, NaiveDate};
use wikimisc::mysql_async::Conn;
use wikimisc::mysql_async::prelude::Queryable;

const SERIES: &str = "tool_uses_total";

/// The complete days to (re)write before `today`: yesterday, and the day
/// before, in case a run was missed or points arrived late.
fn days_to_copy(today: NaiveDate) -> Vec<NaiveDate> {
    [2, 1].iter().filter_map(|n| today.checked_sub_days(Days::new(*n))).collect()
}

/// Midnight (UTC) of `day`, as a timestamp.
fn day_start(day: NaiveDate) -> i64 {
    day.and_hms_opt(0, 0, 0).map_or(0, |t| t.and_utc().timestamp())
}

/// Copies the uses of the last complete days. A day counts only if the
/// service reported uses for all of it: on the day a service switched from
/// the logger to telemetry, the logger's own count stays.
pub async fn copy(conn: &mut Conn, now: i64) -> Result<()> {
    let today = DateTime::from_timestamp(now, 0).map_or_else(NaiveDate::default, |t| t.date_naive());
    // Each service reporting uses, and since when.
    let services: Vec<(String, i64)> = conn
        .exec(
            "SELECT v.name, MIN(h.hour) FROM dash_service v JOIN dash_series s ON s.service_id=v.id
             JOIN dash_point_hourly h ON h.series_id=s.id WHERE s.name=? GROUP BY v.name",
            (SERIES,),
        )
        .await?;
    for day in days_to_copy(today) {
        let start = day_start(day);
        for (service, _) in services.iter().filter(|(_, since)| *since <= start) {
            let uses: Vec<(Option<String>, Option<f64>)> = conn
                .exec(
                    "SELECT JSON_VALUE(s.labels, '$.tool'), SUM(h.delta_sum) FROM dash_service v
                     JOIN dash_series s ON s.service_id=v.id JOIN dash_point_hourly h ON h.series_id=s.id
                     WHERE v.name=? AND s.name=? AND h.hour>=? AND h.hour<? GROUP BY s.labels",
                    (service, SERIES, start, start + DAY),
                )
                .await?;
            for (tool, count) in uses {
                // Counters are whole numbers, stored as floating point.
                let count = count.unwrap_or_default().max(0.0).round() as u64;
                if let Some(tool) = tool.filter(|_| count > 0) {
                    set_uses(conn, service, &tool, day_number(day), count).await?;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_days_only() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
        let days: Vec<String> = days_to_copy(today).iter().map(|d| d.to_string()).collect();
        assert_eq!(days, ["2026-10-30", "2026-10-31"]);
        assert_eq!(day_start(NaiveDate::from_ymd_opt(2026, 10, 27).unwrap()), 1_793_059_200);
    }
}
