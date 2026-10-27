//! The usage dashboard: what the logger (`logger.php`) counted, per tool and
//! day, for the buggregator front-end. Coarser than the service dashboard:
//! uses per day, not requests per minute, but for all tools that log.

use anyhow::Result;
use chrono::{Days, NaiveDate};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use wikimisc::mysql_async::Conn;
use wikimisc::mysql_async::prelude::Queryable;

/// The choice of ranges, by name, in days.
pub const RANGES: &[(&str, u64)] = &[("7d", 7), ("30d", 30), ("90d", 90), ("365d", 365)];

/// The range named `name`, in days; 30 days if there is no such name.
pub fn range(name: &str) -> u64 {
    RANGES.iter().find(|(n, _)| *n == name).map_or(30, |(_, days)| *days)
}

/// The days of a range ending on day `last`, and those of the period before it.
#[derive(Debug, PartialEq, Eq)]
struct Period {
    first: NaiveDate,
    last: NaiveDate,
    previous_first: NaiveDate,
}

impl Period {
    fn new(days: u64, last: NaiveDate) -> Self {
        let first = last.checked_sub_days(Days::new(days.saturating_sub(1))).unwrap_or(last);
        let previous_first = first.checked_sub_days(Days::new(days)).unwrap_or(first);
        Self { first, last, previous_first }
    }

    /// Every day of the range, as the logger writes it.
    fn days(&self) -> Vec<u32> {
        self.first.iter_days().take_while(|d| *d <= self.last).map(day_number).collect()
    }
}

/// `2026-10-27` as `20261027`.
fn day_number(day: NaiveDate) -> u32 {
    day.format("%Y%m%d").to_string().parse().unwrap_or_default()
}

/// `20261027` as a timestamp (midnight UTC), for the charts.
fn timestamp(day: u32) -> i64 {
    let date = NaiveDate::from_ymd_opt((day / 10_000) as i32, day / 100 % 100, day % 100);
    date.and_then(|d| d.and_hms_opt(0, 0, 0)).map_or(0, |d| d.and_utc().timestamp())
}

/// Daily counts as chart points, with 0 for days without uses.
fn points(days: &[u32], counts: &BTreeMap<u32, u64>) -> Vec<Value> {
    days.iter().map(|d| json!([timestamp(*d), counts.get(d).copied().unwrap_or_default()])).collect()
}

/// Rows of (name, day, uses), for the range and the period before it.
#[derive(Debug, Default)]
struct Counts {
    /// Per name: uses per day in the range.
    current: BTreeMap<String, BTreeMap<u32, u64>>,
    /// Per name: uses in the period before.
    previous: BTreeMap<String, u64>,
}

impl Counts {
    fn new(rows: Vec<(String, u32, u64)>, period: &Period) -> Self {
        let first = day_number(period.first);
        let mut counts = Self::default();
        for (name, day, uses) in rows {
            match day >= first {
                true => *counts.current.entry(name).or_default().entry(day).or_default() += uses,
                false => *counts.previous.entry(name).or_default() += uses,
            }
        }
        counts
    }

    /// Per name, most used first: uses, the period before, days with uses,
    /// daily points; and the daily totals.
    fn summary(&self, days: &[u32], last_used: &BTreeMap<String, u32>) -> (Vec<Value>, Vec<Value>) {
        let mut totals: BTreeMap<u32, u64> = BTreeMap::new();
        let mut names: Vec<String> = self.current.keys().chain(self.previous.keys()).cloned().collect();
        names.sort();
        names.dedup();
        let mut rows: Vec<(u64, Value)> = names
            .into_iter()
            .map(|name| {
                let daily = self.current.get(&name).cloned().unwrap_or_default();
                for (day, uses) in &daily {
                    *totals.entry(*day).or_default() += uses;
                }
                let uses: u64 = daily.values().sum();
                let row = json!({
                    "name": name,
                    "uses": uses,
                    "previous": self.previous.get(&name).copied().unwrap_or_default(),
                    "days_used": daily.values().filter(|n| **n > 0).count(),
                    "last_used": last_used.get(&name).map(|d| timestamp(*d)),
                    "points": points(days, &daily),
                });
                (uses, row)
            })
            .collect();
        rows.sort_by_key(|(uses, _)| std::cmp::Reverse(*uses));
        (rows.into_iter().map(|(_, row)| row).collect(), points(days, &totals))
    }
}

/// All tools in a range of `days` ending on day `last`.
pub async fn overview(conn: &mut Conn, days: u64, last: NaiveDate) -> Result<Value> {
    let period = Period::new(days, last);
    let rows: Vec<(String, u32, u64)> = conn
        .exec(
            "SELECT lt.name, u.date, SUM(u.used) FROM logging_uses u JOIN logging_tools lt ON lt.id=u.tool_id
             WHERE u.date BETWEEN ? AND ? GROUP BY lt.name, u.date",
            (day_number(period.previous_first), day_number(period.last)),
        )
        .await?;
    let last_used: BTreeMap<String, u32> = conn
        .query(
            "SELECT lt.name, MAX(u.date) FROM logging_uses u JOIN logging_tools lt ON lt.id=u.tool_id GROUP BY lt.name",
        )
        .await?
        .into_iter()
        .collect();
    let days_list = period.days();
    let (tools, totals) = Counts::new(rows, &period).summary(&days_list, &last_used);
    Ok(json!({"days": days, "from": timestamp(day_number(period.first)), "to": timestamp(day_number(period.last)),
              "totals": totals, "tools": tools}))
}

/// One tool in a range: the same per method (for the tools here, per sub-tool).
pub async fn tool(conn: &mut Conn, name: &str, days: u64, last: NaiveDate) -> Result<Value> {
    let period = Period::new(days, last);
    let rows: Vec<(String, u32, u64)> = conn
        .exec(
            "SELECT lt.method, u.date, SUM(u.used) FROM logging_uses u JOIN logging_tools lt ON lt.id=u.tool_id
             WHERE lt.name=? AND u.date BETWEEN ? AND ? GROUP BY lt.method, u.date",
            (name, day_number(period.previous_first), day_number(period.last)),
        )
        .await?;
    let last_used: BTreeMap<String, u32> = conn
        .exec(
            "SELECT lt.method, MAX(u.date) FROM logging_uses u JOIN logging_tools lt ON lt.id=u.tool_id
             WHERE lt.name=? GROUP BY lt.method",
            (name,),
        )
        .await?
        .into_iter()
        .collect();
    let (methods, totals) = Counts::new(rows, &period).summary(&period.days(), &last_used);
    Ok(json!({"name": name, "methods": methods, "totals": totals}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn periods_and_days() {
        let period = Period::new(7, date("2026-10-27"));
        assert_eq!(
            period,
            Period { first: date("2026-10-21"), last: date("2026-10-27"), previous_first: date("2026-10-14") }
        );
        assert_eq!(period.days(), [20261021, 20261022, 20261023, 20261024, 20261025, 20261026, 20261027]);
        assert_eq!(timestamp(20261027), 1_793_059_200);
        assert_eq!((range("90d"), range("nonsense")), (90, 30));
    }

    #[test]
    fn summaries() {
        let period = Period::new(3, date("2026-10-27"));
        let rows = vec![
            ("fist".to_string(), 20261025, 5),
            ("fist".to_string(), 20261027, 2),
            ("fist".to_string(), 20261023, 10),
            ("icommons".to_string(), 20261027, 9),
            ("gone".to_string(), 20261022, 4),
        ];
        let last_used = BTreeMap::from([("fist".to_string(), 20261027)]);
        let (tools, totals) = Counts::new(rows, &period).summary(&period.days(), &last_used);
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["icommons", "fist", "gone"]);
        let fist = &tools[1];
        assert_eq!(
            (fist["uses"].as_u64(), fist["previous"].as_u64(), fist["days_used"].as_u64()),
            (Some(7), Some(10), Some(2))
        );
        assert_eq!(fist["points"][1], json!([timestamp(20261026), 0]));
        assert_eq!(fist["last_used"], json!(timestamp(20261027)));
        assert_eq!(tools[2]["uses"], 0);
        assert_eq!(
            totals,
            vec![json!([timestamp(20261025), 5]), json!([timestamp(20261026), 0]), json!([timestamp(20261027), 11])]
        );
    }
}
