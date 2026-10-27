//! The tool monitor: checks that tools are working, writes the result to
//! `tool_status.html`, and mails new alerts. Tools with a service on the
//! dashboard are left to the dashboard's own alerts (`dashboard_alerts`).

use super::mail;
use crate::state::AppState;
use anyhow::Result;
use askama::Template;
use serde_json::Value;
use std::time::Duration;
use wikimisc::mysql_async::prelude::Queryable;

const TIMEOUT: Duration = Duration::from_secs(60);

/// Tool name and its alerts (none: nominal), in the order checked.
#[derive(Debug, Default)]
struct Alerts(Vec<(String, Vec<String>)>);

impl Alerts {
    fn tool(&mut self, name: &str) -> &mut Vec<String> {
        let index = match self.0.iter().position(|(tool, _)| tool == name) {
            Some(index) => index,
            None => {
                self.0.push((name.to_string(), vec![]));
                self.0.len() - 1
            }
        };
        &mut self.0[index].1
    }

    fn has(&self, name: &str) -> bool {
        self.0.iter().any(|(tool, _)| tool == name)
    }
}

#[derive(Template)]
#[template(path = "tool_status.html")]
struct StatusPage<'a> {
    title: &'static str,
    alerts: &'a Alerts,
    updated: String,
}

#[derive(Template)]
#[template(path = "tool_status_mail.html")]
struct AlertMail<'a> {
    alerts: &'a Alerts,
}

pub async fn run(state: &AppState) -> Result<()> {
    let client = reqwest::Client::builder().user_agent(crate::state::USER_AGENT).timeout(TIMEOUT).build()?;
    let mut alerts = Alerts::default();
    reinheitsgebot(&client, &mut alerts).await;
    web_status(state, &mut alerts).await?;

    let now = chrono::Utc::now();
    let page =
        StatusPage { title: "Tool status", alerts: &alerts, updated: now.format("%Y-%m-%d %H:%M:%S UTC").to_string() };
    tokio::fs::write(&state.config.monitor.status_page, page.render()?).await?;

    let config = &state.config.monitor;
    let failing = Alerts(alerts.0.into_iter().filter(|(_, a)| !a.is_empty()).collect());
    let message = if failing.0.is_empty() { String::new() } else { AlertMail { alerts: &failing }.render()? };
    let last = tokio::fs::read_to_string(&config.state_file).await.unwrap_or_default();
    if message != last {
        if !message.is_empty() {
            for (tool, alerts) in &failing.0 {
                tracing::warn!("{tool}: {}", alerts.join("; "));
            }
            mail::send(config, "Tool monitor alert", &message).await?;
        }
        tokio::fs::write(&config.state_file, &message).await?;
    }
    Ok(())
}

async fn json(client: &reqwest::Client, url: &str) -> Option<Value> {
    client.get(url).send().await.ok()?.error_for_status().ok()?.json().await.ok()
}

async fn reinheitsgebot(client: &reqwest::Client, alerts: &mut Alerts) {
    let alerts = alerts.tool("Reinheitsgebot");
    let url = "https://www.wikidata.org/w/api.php?action=query&list=users&ususers=Reinheitsgebot&usprop=blockinfo&format=json";
    let Some(json) = json(client, url).await else { return alerts.push("Can't get user info from API".to_string()) };
    let user = &json["query"]["users"][0];
    if user.get("blockid").is_some() {
        let text = |key: &str| user[key].as_str().unwrap_or_default().to_string();
        alerts.push(format!("Blocked for '{}' since {}", text("blockreason"), text("blockedtimestamp")));
    }
}

/// Tools whose web address answered with something other than 200, by the
/// `webstatus` job; but not those with a service on the dashboard.
async fn web_status(state: &AppState, alerts: &mut Alerts) -> Result<()> {
    let mut conn = state.tool_db.conn().await?;
    let broken: Vec<String> = conn
        .query(
            "SELECT DISTINCT subdomain FROM tool WHERE has_webservice=1 AND is_active=1 AND subdomain!=''
             AND (http_status!=200 OR http_status IS NULL)
             AND subdomain NOT IN (SELECT t.subdomain FROM dash_service s JOIN tool t ON t.id=s.tool_id
                                   WHERE s.is_active=1 AND t.subdomain!='')",
        )
        .await?;
    for subdomain in &broken {
        if !alerts.has(subdomain) {
            alerts.tool(subdomain).push("Buggregator reports non-200 status".to_string());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alerts_keep_order() {
        let mut alerts = Alerts::default();
        alerts.tool("B");
        alerts.tool("A").push("x".to_string());
        alerts.tool("B").push("y".to_string());
        assert_eq!(alerts.0, vec![("B".to_string(), vec!["y".to_string()]), ("A".to_string(), vec!["x".to_string()])]);
        assert!(alerts.has("A") && !alerts.has("C"));
    }
}
