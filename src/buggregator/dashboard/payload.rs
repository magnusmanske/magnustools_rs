//! A push from a service, checked: the wire format is `wikimisc`'s
//! `telemetry_wire`; this applies the dashboard's rules to it. A broken
//! instance block rejects the push; a bad metric or event is skipped and
//! counted, so one bad metric does not cost the rest.

use wikimisc::telemetry_wire::{
    self as wire, MAX_EVENTS, MAX_HOST, MAX_METRICS, MAX_TEXT, MAX_VERSION, MetricKind, is_valid_component,
    is_valid_metric, is_valid_uid,
};

/// Stored label sets are sorted JSON of at most this many bytes.
const MAX_LABELS_JSON: usize = 255;
/// Event times come from the service's clock; older than this is implausible.
const MAX_EVENT_AGE: i64 = 86_400;

#[derive(Debug, PartialEq)]
pub struct Metric {
    pub name: String,
    /// Sorted JSON, `{}` without labels.
    pub labels: String,
    pub kind: MetricKind,
    pub value: f64,
}

#[derive(Debug, PartialEq)]
pub struct Event {
    pub ts: i64,
    pub level: &'static str,
    pub message: String,
}

#[derive(Debug, PartialEq)]
pub struct Exit {
    pub reason: &'static str,
    pub detail: String,
}

#[derive(Debug, PartialEq)]
pub struct Push {
    pub uid: String,
    pub component: String,
    pub host: String,
    pub version: String,
    pub started: i64,
    pub metrics: Vec<Metric>,
    pub events: Vec<Event>,
    pub exit: Option<Exit>,
    pub skipped: usize,
}

impl Push {
    /// The error is the message for the 400 answer.
    pub fn parse(body: &[u8], now: i64) -> Result<Self, String> {
        let json: serde_json::Value =
            serde_json::from_slice(body).map_err(|_| "Body is not a JSON object".to_string())?;
        if json["v"] != wire::VERSION {
            return Err("Unsupported payload version".to_string());
        }
        let payload: wire::Payload = serde_json::from_value(json).map_err(|e| format!("Invalid payload: {e}"))?;
        let instance = payload.instance;
        if !is_valid_uid(&instance.uid) {
            return Err("Invalid instance.uid".to_string());
        }
        if !is_valid_component(&instance.component) {
            return Err("Invalid instance.component".to_string());
        }
        let started = i64::try_from(instance.started).ok().filter(|s| *s > 0 && *s <= now).unwrap_or(now);

        let mut skipped = payload.metrics.len().saturating_sub(MAX_METRICS);
        // Clients put their process readings first, so cutting off the end keeps those.
        let mut metrics: Vec<Metric> = vec![];
        for metric in payload.metrics.into_iter().take(MAX_METRICS) {
            let labels = serde_json::to_string(&metric.labels).unwrap_or_default();
            if !is_valid_metric(&metric) || labels.len() > MAX_LABELS_JSON {
                skipped += 1;
                continue;
            }
            let metric = Metric { name: metric.name, labels, kind: metric.kind, value: metric.value };
            // A repeat replaces the earlier value.
            match metrics.iter_mut().find(|m| m.name == metric.name && m.labels == metric.labels) {
                Some(earlier) => *earlier = metric,
                None => metrics.push(metric),
            }
        }

        skipped += payload.events.len().saturating_sub(MAX_EVENTS);
        let mut events = vec![];
        for event in payload.events.into_iter().take(MAX_EVENTS) {
            let message = text(&event.message, MAX_TEXT);
            if message.is_empty() {
                skipped += 1;
                continue;
            }
            let ts = i64::try_from(event.ts).ok().filter(|ts| *ts >= now - MAX_EVENT_AGE && *ts <= now).unwrap_or(now);
            events.push(Event { ts, level: event.level.as_str(), message });
        }

        Ok(Self {
            uid: instance.uid,
            component: instance.component,
            host: text(&instance.host, MAX_HOST),
            version: text(&instance.version, MAX_VERSION),
            started,
            metrics,
            events,
            exit: payload.exit.map(|e| Exit { reason: e.reason.as_str(), detail: text(&e.detail, MAX_TEXT) }),
            skipped,
        })
    }

    /// An unlabelled metric's value, as a whole number.
    pub fn metric_value(&self, name: &str) -> Option<u64> {
        let value = self.metrics.iter().find(|m| m.name == name && m.labels == "{}")?.value;
        // Byte counts: non-negative and well within u64.
        (value >= 0.0).then_some(value as u64)
    }
}

/// Trimmed, at most `max` characters.
fn text(s: &str, max: usize) -> String {
    s.trim().chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NOW: i64 = 1_760_000_000;

    fn push(payload: serde_json::Value) -> Result<Push, String> {
        Push::parse(payload.to_string().as_bytes(), NOW)
    }

    fn instance() -> serde_json::Value {
        json!({"uid": "pod-1-2", "component": "web", "host": " pod ", "version": "1.0", "started": NOW - 10})
    }

    #[test]
    fn valid_push() {
        let p = push(json!({
            "v": 1, "instance": instance(),
            "metrics": [
                {"name": "x_total", "kind": "counter", "labels": {"b": "2", "a": "1"}, "value": 3},
                {"name": "bad-name", "kind": "gauge", "value": 1},
                {"name": "x_total", "kind": "counter", "labels": {"a": "1", "b": "2"}, "value": 4},
                {"name": "neg", "kind": "counter", "value": -1},
            ],
            "events": [{"ts": NOW - 5, "level": "warn", "message": " hm "}, {"ts": 1, "level": "info", "message": "old"},
                       {"ts": NOW, "level": "info", "message": "  "}],
            "exit": {"reason": "meteor", "detail": "boom"}
        }))
        .unwrap();
        assert_eq!((p.host.as_str(), p.started), ("pod", NOW - 10));
        assert_eq!(p.metrics.len(), 1);
        assert_eq!((p.metrics[0].labels.as_str(), p.metrics[0].value), (r#"{"a":"1","b":"2"}"#, 4.0));
        assert_eq!(p.skipped, 3);
        assert_eq!(
            p.events,
            vec![
                Event { ts: NOW - 5, level: "warn", message: "hm".into() },
                Event { ts: NOW, level: "info", message: "old".into() },
            ]
        );
        assert_eq!(p.exit, Some(Exit { reason: "other", detail: "boom".into() }));
    }

    #[test]
    fn rejected_pushes() {
        assert!(push(json!({"v": 2, "instance": instance()})).unwrap_err().contains("version"));
        let mut bad = instance();
        bad["component"] = json!("Web");
        assert_eq!(push(json!({"v": 1, "instance": bad})).unwrap_err(), "Invalid instance.component");
        let mut bad = instance();
        bad["uid"] = json!("a b");
        assert_eq!(push(json!({"v": 1, "instance": bad})).unwrap_err(), "Invalid instance.uid");
        assert!(Push::parse(b"not json", NOW).is_err());
    }

    #[test]
    fn implausible_start_is_now() {
        let mut future = instance();
        future["started"] = json!(NOW + 100);
        assert_eq!(push(json!({"v": 1, "instance": future})).unwrap().started, NOW);
    }
}
