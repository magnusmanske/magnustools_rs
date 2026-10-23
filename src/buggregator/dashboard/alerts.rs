//! Which components of the services on the dashboard are offline: no
//! process has reported for more than `OFFLINE_AFTER`. The `dashboard-alerts`
//! job mails them, once per outage.

use chrono::DateTime;
use serde_json::Value;

/// Seconds without a report before a component counts as offline.
pub const OFFLINE_AFTER: i64 = 600;

#[derive(Debug, PartialEq, Eq)]
pub struct Offline {
    /// `service/component`, to recognise the same outage in the next run.
    pub key: String,
    /// What to tell, e.g. `Mix'n'match (web): no report since 2026-10-23 17:05 UTC`.
    pub text: String,
}

/// From the dashboard overview ([`super::query::overview`]): the offline
/// components, except those whose outage was acknowledged on the dashboard.
pub fn offline(overview: &Value, now: i64) -> Vec<Offline> {
    let mut out = vec![];
    for service in overview.as_array().into_iter().flatten() {
        let name = service["name"].as_str().unwrap_or_default();
        let label = service["label"].as_str().unwrap_or(name);
        for (component, c) in service["components"].as_object().into_iter().flatten() {
            if !matches!(c["status"].as_str(), Some("down" | "stopped")) {
                continue;
            }
            let Some(last_seen) = c["current"]["last_seen"].as_i64() else { continue };
            let acknowledged = c["acks"]["status"].as_i64().is_some_and(|acked_at| acked_at >= last_seen);
            if now - last_seen <= OFFLINE_AFTER || acknowledged {
                continue;
            }
            let since = DateTime::from_timestamp(last_seen, 0)
                .map_or_else(String::new, |t| t.format("%Y-%m-%d %H:%M UTC").to_string());
            let reason = match c["current"]["end_reason"].as_str() {
                Some(reason) => format!(" (stopped: {reason})"),
                None => String::new(),
            };
            out.push(Offline {
                key: format!("{name}/{component}"),
                text: format!("{label} ({component}): no report since {since}{reason}"),
            });
        }
    }
    out
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
}
