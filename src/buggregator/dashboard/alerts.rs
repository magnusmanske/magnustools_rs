//! What the dashboard alerts on: components whose process has not reported
//! for more than `OFFLINE_AFTER`, and web addresses that do not answer with
//! 200. The `dashboard-alerts` job mails them, once per outage and again each
//! day while it lasts.

use chrono::DateTime;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use wikimisc::mysql_async::Conn;
use wikimisc::mysql_async::prelude::Queryable;

/// Seconds without a report before a component counts as offline.
pub const OFFLINE_AFTER: i64 = super::INACTIVE_AFTER;

/// Seconds before a still-failing alert is mailed again.
pub const REMIND_AFTER: i64 = 86_400;

/// One thing to tell about.
#[derive(Debug, PartialEq, Eq)]
pub struct Alert {
    /// `service/component` or `web/<url>`, to recognise it in the next run.
    pub key: String,
    /// What to name it in the mail's subject, e.g. `Mix'n'match (web)`.
    pub name: String,
    /// What to tell, e.g. `Mix'n'match (web): no report since 2026-10-23 17:05 UTC`.
    pub text: String,
}

/// From the dashboard overview ([`super::query::overview`]): the offline
/// components, except those whose outage was acknowledged on the dashboard.
pub fn offline(overview: &Value, now: i64) -> Vec<Alert> {
    let mut out = vec![];
    for service in overview.as_array().into_iter().flatten() {
        let service_name = service["name"].as_str().unwrap_or_default();
        let label = service["label"].as_str().unwrap_or(service_name);
        for (component, c) in service["components"].as_object().into_iter().flatten() {
            if !matches!(c["status"].as_str(), Some("down" | "stopped")) {
                continue;
            }
            let Some(last_seen) = c["current"]["last_seen"].as_i64() else { continue };
            let acknowledged = c["acks"]["status"].as_i64().is_some_and(|acked_at| acked_at >= last_seen);
            if now - last_seen <= OFFLINE_AFTER || acknowledged {
                continue;
            }
            // A planned stop or an outside replacement is not an outage.
            if c["current"]["end_reason"].as_str().is_some_and(|reason| super::EXPECTED_END_REASONS.contains(&reason)) {
                continue;
            }
            let since = DateTime::from_timestamp(last_seen, 0)
                .map_or_else(String::new, |t| t.format("%Y-%m-%d %H:%M UTC").to_string());
            let reason = match c["current"]["end_reason"].as_str() {
                Some(reason) => format!(" (stopped: {reason})"),
                None => String::new(),
            };
            let name = format!("{label} ({component})");
            out.push(Alert {
                key: format!("{service_name}/{component}"),
                text: format!("{name}: no report since {since}{reason}"),
                name,
            });
        }
    }
    out
}

/// A web address that does not answer with 200, for the dashboard and the
/// alert mail. One per address: tools that share one (Reasonator has several
/// entries) are one alert.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct WebAlert {
    /// The tool named like its subdomain, else the first by name.
    pub label: String,
    pub url: String,
    /// 0: no answer at all.
    pub status: u16,
    /// When this answer started, if that was seen.
    pub since: Option<i64>,
    pub severity: &'static str,
    /// The dashboard service using it, for the link to its card.
    pub service: Option<String>,
    /// How many tools answer at this address.
    pub tools: usize,
}

impl WebAlert {
    /// For the state file, along with `service/component` keys.
    pub fn key(&self) -> String {
        format!("web/{}", self.url)
    }

    /// For the alert mail.
    pub fn text(&self) -> String {
        let what = if self.status == 0 { "no answer".to_string() } else { format!("HTTP {}", self.status) };
        let since = self
            .since
            .and_then(|t| DateTime::from_timestamp(t, 0))
            .map_or_else(String::new, |t| format!(" since {}", t.format("%Y-%m-%d %H:%M UTC")));
        let tools = if self.tools > 1 { format!(", {} tools", self.tools) } else { String::new() };
        format!("{} ({}{tools}): {what}{since}", self.label, self.url)
    }

    pub fn alert(&self) -> Alert {
        Alert { key: self.key(), name: self.label.clone(), text: self.text() }
    }
}

/// A `tool` row for [`web`]: name, subdomain, URL (its own or its service's;
/// `None` when neither is set), service, status, and since.
type Row = (String, String, Option<String>, Option<String>, i64, Option<i64>);

/// From the `tool` table, which the `webstatus` job fills: the tools whose
/// web address does not answer with 200. A checked address that never
/// answered is `0`; `NULL` means there was nothing to check.
pub async fn web(conn: &mut Conn) -> anyhow::Result<Vec<WebAlert>> {
    let rows: Vec<Row> = conn
        .query(
            "SELECT t.name, t.subdomain, COALESCE(NULLIF(s.url,''), NULLIF(t.url,'')), s.name,
                    t.http_status, t.http_status_since
             FROM tool t LEFT JOIN dash_service s ON s.tool_id=t.id AND s.is_active=1
             WHERE t.is_active=1 AND (t.has_webservice=1 OR s.id IS NOT NULL)
               AND t.http_status IS NOT NULL AND t.http_status<>200
             ORDER BY t.name",
        )
        .await?;
    let rows = rows
        .into_iter()
        .map(|(name, subdomain, url, service, status, since)| WebRow {
            name,
            subdomain,
            url: url.unwrap_or_default(),
            service,
            status,
            since,
        })
        .collect();
    Ok(group(rows))
}

/// One `tool` row, as [`web`] reads it.
#[derive(Debug)]
struct WebRow {
    name: String,
    subdomain: String,
    url: String,
    service: Option<String>,
    status: i64,
    since: Option<i64>,
}

/// One alert per address. The tool named like the subdomain is the service
/// itself; its other entries (`geneawiki`, `widee`, …) are aliases.
fn group(rows: Vec<WebRow>) -> Vec<WebAlert> {
    let mut by_url: BTreeMap<String, Vec<WebRow>> = BTreeMap::new();
    for row in rows {
        let Some(url) = crate::buggregator::address(&row.subdomain, &row.url) else { continue };
        by_url.entry(url).or_default().push(row);
    }
    by_url
        .into_iter()
        .map(|(url, mut rows)| {
            rows.sort_by(|a, b| (a.name != a.subdomain, &a.name).cmp(&(b.name != b.subdomain, &b.name)));
            let primary = &rows[0];
            let status = u16::try_from(primary.status).unwrap_or(0);
            WebAlert {
                label: primary.name.clone(),
                url,
                status,
                // The earliest failure among the aliases; they normally agree.
                since: rows.iter().filter_map(|r| r.since).min(),
                severity: if status == 0 || status >= 500 { "critical" } else { "warning" },
                service: rows.iter().find_map(|r| r.service.clone()),
                tools: rows.len(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NOW: i64 = 1_790_000_000;

    fn overview(status: &str, last_seen: i64, end_reason: Option<&str>, acked_at: Option<i64>) -> Value {
        json!([{
            "name": "mixnmatch", "label": "Mix'n'match",
            "components": {"web": {
                "status": status,
                "current": {"last_seen": last_seen, "end_reason": end_reason},
                "acks": acked_at.map_or_else(|| json!({}), |t| json!({"status": t})),
            }}
        }])
    }

    #[test]
    fn offline_after_ten_minutes() {
        assert!(offline(&overview("up", NOW - 30, None, None), NOW).is_empty());
        assert!(offline(&overview("down", NOW - OFFLINE_AFTER, None, None), NOW).is_empty());
        let down = offline(&overview("down", NOW - OFFLINE_AFTER - 1, None, None), NOW);
        assert_eq!(down.len(), 1);
        assert_eq!(down[0].key, "mixnmatch/web");
        assert_eq!(down[0].name, "Mix'n'match (web)");
        assert!(down[0].text.starts_with("Mix'n'match (web): no report since "), "{}", down[0].text);
        let stopped = offline(&overview("stopped", NOW - 3600, Some("crash"), None), NOW);
        assert!(stopped[0].text.ends_with("(stopped: crash)"));
    }

    #[test]
    fn acknowledged_outages_stay_quiet() {
        assert!(offline(&overview("down", NOW - 3600, None, Some(NOW - 3000)), NOW).is_empty());
        // An acknowledgement of an earlier outage does not cover this one.
        assert_eq!(offline(&overview("down", NOW - 3600, None, Some(NOW - 7200)), NOW).len(), 1);
    }

    #[test]
    fn planned_ends_stay_quiet() {
        for reason in ["shutdown", "exit", "vanished"] {
            assert!(offline(&overview("stopped", NOW - 3600, Some(reason), None), NOW).is_empty(), "{reason}");
        }
        assert_eq!(offline(&overview("stopped", NOW - 3600, Some("crash"), None), NOW).len(), 1);
    }

    fn row(name: &str, subdomain: &str, status: i64, since: Option<i64>) -> WebRow {
        WebRow {
            name: name.into(),
            subdomain: subdomain.into(),
            url: "https://reasonator.toolforge.org/".into(),
            service: (name == "reasonator").then(|| "reasonator".to_string()),
            status,
            since,
        }
    }

    #[test]
    fn one_alert_per_address_under_its_own_name() {
        let alerts = group(vec![
            row("geneawiki", "reasonator", 503, Some(2_000)),
            row("reasonator", "reasonator", 503, Some(1_000)),
        ]);
        assert_eq!(alerts.len(), 1);
        let alert = &alerts[0];
        assert_eq!(alert.label, "reasonator");
        assert_eq!(alert.tools, 2);
        assert_eq!(alert.since, Some(1_000));
        assert_eq!(alert.service.as_deref(), Some("reasonator"));
        assert_eq!(alert.severity, "critical");
        assert_eq!(alert.key(), "web/https://reasonator.toolforge.org/");
        assert_eq!(
            alert.text(),
            "reasonator (https://reasonator.toolforge.org/, 2 tools): HTTP 503 since 1970-01-01 00:16 UTC"
        );
    }

    #[test]
    fn no_answer_is_critical_and_a_client_error_is_not() {
        assert_eq!(group(vec![row("x", "", 0, None)])[0].severity, "critical");
        assert_eq!(group(vec![row("x", "", 404, None)])[0].severity, "warning");
        assert!(group(vec![row("x", "", 0, None)])[0].text().contains(": no answer"));
    }

    #[test]
    fn web_alerts_serialise_for_the_front_end() {
        let alert = &group(vec![row("reasonator", "reasonator", 503, Some(1_000))])[0];
        let json = serde_json::to_value(alert).unwrap();
        assert_eq!(json["label"], "reasonator");
        assert_eq!(json["status"], 503);
        assert_eq!(json["severity"], "critical");
        assert_eq!(json["service"], "reasonator");
        assert_eq!(json["tools"], 1);
    }
}
