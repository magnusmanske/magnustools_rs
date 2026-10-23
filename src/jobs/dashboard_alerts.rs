//! Mails when a service on the dashboard has been offline for more than
//! ten minutes; once per outage, remembered in `monitor.dashboard_state_file`.

use super::mail;
use crate::buggregator::dashboard::{self, alerts, query};
use crate::state::AppState;
use anyhow::Result;
use askama::Template;
use std::collections::BTreeSet;

#[derive(Template)]
#[template(path = "dashboard_alert_mail.html")]
struct AlertMail<'a> {
    offline: &'a [&'a alerts::Offline],
    dashboard_url: String,
}

pub async fn run(state: &AppState) -> Result<()> {
    let mut conn = state.tool_db.conn().await?;
    let overview = query::overview(&mut conn, dashboard::now()).await?;
    let offline = alerts::offline(&overview, dashboard::now());

    let config = &state.config.monitor;
    let text = tokio::fs::read_to_string(&config.dashboard_state_file).await.unwrap_or_default();
    let known: BTreeSet<&str> = text.lines().collect();
    let new: Vec<&alerts::Offline> = offline.iter().filter(|o| !known.contains(o.key.as_str())).collect();
    if !new.is_empty() {
        for o in &new {
            tracing::warn!("{}", o.text);
        }
        let names: Vec<&str> = new.iter().map(|o| o.key.as_str()).collect();
        let dashboard_url = format!("{}/buggregator/#/dashboard", state.config.public_url);
        let mail = AlertMail { offline: &new, dashboard_url }.render()?;
        mail::send(config, &format!("Service offline: {}", names.join(", ")), &mail).await?;
    }
    // Recovered outages drop out, so that the next one is mailed again.
    let keys: Vec<&str> = offline.iter().map(|o| o.key.as_str()).collect();
    tokio::fs::write(&config.dashboard_state_file, keys.join("\n")).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mail_lists_the_outages() {
        let offline = alerts::Offline { key: "a/web".into(), text: "A <b> (web): no report since …".into() };
        let html = AlertMail { offline: &[&offline], dashboard_url: "https://x/buggregator/#/dashboard".into() }
            .render()
            .unwrap();
        assert!(html.contains("<li>A &#60;b&#62; (web): no report since …</li>"), "{html}");
        assert!(html.contains("https://x/buggregator/#/dashboard"));
    }
}
