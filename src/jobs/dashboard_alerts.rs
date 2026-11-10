//! Mails when something on the dashboard needs attention: a service offline
//! for more than ten minutes, or a web address that does not answer with 200.
//! Once per outage, and again each day while it lasts; the keys are kept in
//! `monitor.dashboard_state_file`.

use super::mail;
use crate::buggregator::dashboard::{self, alerts, query};
use crate::state::AppState;
use anyhow::Result;
use askama::Template;
use std::collections::BTreeMap;

#[derive(Template)]
#[template(path = "dashboard_alert_mail.html")]
struct AlertMail<'a> {
    alerts: &'a [&'a alerts::Alert],
    dashboard_url: String,
}

pub async fn run(state: &AppState) -> Result<()> {
    let mut conn = state.tool_db.conn().await?;
    let now = dashboard::now();
    let overview = query::overview(&mut conn, now).await?;
    let mut current = alerts::offline(&overview, now);
    // Web addresses of tools that have a service here; the monitor watches the others.
    current.extend(alerts::web(&mut conn).await?.iter().filter(|w| w.service.is_some()).map(alerts::WebAlert::alert));
    current.sort_by(|a, b| a.key.cmp(&b.key));

    let config = &state.config.monitor;
    let sent = parse_state(&tokio::fs::read_to_string(&config.dashboard_state_file).await.unwrap_or_default());
    let new: Vec<&alerts::Alert> = current.iter().filter(|a| is_due(&a.key, &sent, now)).collect();
    if !new.is_empty() {
        for alert in &new {
            tracing::warn!("{}", alert.text);
        }
        let names: Vec<&str> = new.iter().map(|a| a.name.as_str()).collect();
        let dashboard_url = format!("{}/buggregator/#/dashboard", state.config.public_url);
        let mail = AlertMail { alerts: &new, dashboard_url }.render()?;
        mail::send(config, &format!("Service alert: {}", names.join(", ")), &mail).await?;
    }
    // Recovered alerts drop out, so that the next one is mailed again.
    let text = state_file(&current, &sent, now);
    tokio::fs::write(&config.dashboard_state_file, text).await?;
    Ok(())
}

/// Whether an alert should be mailed now: new, or last mailed `REMIND_AFTER` ago.
fn is_due(key: &str, sent: &BTreeMap<String, i64>, now: i64) -> bool {
    sent.get(key).is_none_or(|last| now - last >= alerts::REMIND_AFTER)
}

/// One `key<TAB>last mailed` line per alert; keys and URLs hold no tabs.
fn parse_state(text: &str) -> BTreeMap<String, i64> {
    text.lines()
        .filter_map(|line| {
            let (key, last) = line.rsplit_once('\t')?;
            Some((key.to_string(), last.trim().parse().ok()?))
        })
        .collect()
}

fn state_file(current: &[alerts::Alert], sent: &BTreeMap<String, i64>, now: i64) -> String {
    current
        .iter()
        .map(|a| {
            let last = if is_due(&a.key, sent, now) { now } else { sent.get(&a.key).copied().unwrap_or(now) };
            format!("{}\t{last}", a.key)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_790_000_000;

    fn alert(key: &str) -> alerts::Alert {
        alerts::Alert { key: key.into(), name: "A (web)".into(), text: "A <b> (web): no report since …".into() }
    }

    #[test]
    fn mail_lists_the_alert() {
        let alert = alert("a/web");
        let html = AlertMail { alerts: &[&alert], dashboard_url: "https://x/buggregator/#/dashboard".into() }
            .render()
            .unwrap();
        assert!(html.contains("<li>A &#60;b&#62; (web): no report since …</li>"), "{html}");
        assert!(html.contains("https://x/buggregator/#/dashboard"));
    }

    #[test]
    fn new_alerts_are_mailed_and_old_ones_reminded_daily() {
        let (a, b) = (alert("a/web"), alert("b/web"));
        let sent = BTreeMap::from([("a/web".to_string(), NOW - 3600)]);
        let new: Vec<&str> =
            [&a, &b].into_iter().filter(|x| is_due(&x.key, &sent, NOW)).map(|x| x.key.as_str()).collect();
        assert_eq!(new, ["b/web"]);
        let sent = BTreeMap::from([("a/web".to_string(), NOW - alerts::REMIND_AFTER)]);
        assert!(is_due(&a.key, &sent, NOW));
    }

    #[test]
    fn the_state_file_survives_a_round_trip() {
        let (a, b) = (alert("a/web"), alert("web/https://x/"));
        let sent = BTreeMap::from([("a/web".to_string(), NOW - 3600)]);
        let text = state_file(&[a, b], &sent, NOW);
        assert_eq!(text, format!("a/web\t{}\nweb/https://x/\t{NOW}", NOW - 3600));
        let parsed = parse_state(&text);
        assert_eq!(parsed.get("a/web"), Some(&(NOW - 3600)));
        assert_eq!(parsed.get("web/https://x/"), Some(&NOW));
    }
}
