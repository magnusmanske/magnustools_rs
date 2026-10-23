//! The tool monitor: checks that the busiest tools are working, writes the
//! result to `tool_status.html`, and mails new alerts.

use crate::buggregator::dashboard::{self, query};
use crate::config::MonitorConfig;
use crate::state::AppState;
use anyhow::{Context, Result};
use askama::Template;
use chrono::{DateTime, NaiveDateTime, Utc};
use lettre::message::header::ContentType;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use serde_json::Value;
use std::time::Duration;
use wikimisc::mysql_async::prelude::Queryable;

const TIMEOUT: Duration = Duration::from_secs(60);
/// Checked by their own rules, not by their web status.
const OWN_CHECKS: &[&str] = &["quickstatements", "petscan", "mix-n-match", "listeriabot"];
const EXPECTED_END_REASONS: &[&str] = &["shutdown", "exit", "vanished"];

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
    quickstatements(&client, &mut alerts).await;
    petscan(&client, &mut alerts).await;
    mixnmatch(&client, &mut alerts).await;
    reinheitsgebot(&client, &mut alerts).await;
    listeriabot(&client, &mut alerts).await;
    web_status(state, &mut alerts).await?;
    dashboard_services(state, &mut alerts).await?;

    let now = Utc::now();
    let page =
        StatusPage { title: "Tool status", alerts: &alerts, updated: now.format("%Y-%m-%d %H:%M:%S UTC").to_string() };
    tokio::fs::write(state.config.public_html.join("tool_status.html"), page.render()?).await?;

    let config = &state.config.monitor;
    let failing = Alerts(alerts.0.into_iter().filter(|(_, a)| !a.is_empty()).collect());
    let message = if failing.0.is_empty() { String::new() } else { AlertMail { alerts: &failing }.render()? };
    let last = tokio::fs::read_to_string(&config.state_file).await.unwrap_or_default();
    if message != last {
        if !message.is_empty() {
            for (tool, alerts) in &failing.0 {
                tracing::warn!("{tool}: {}", alerts.join("; "));
            }
            send(config, &message).await?;
        }
        tokio::fs::write(&config.state_file, &message).await?;
    }
    Ok(())
}

async fn send(config: &MonitorConfig, message: &str) -> Result<()> {
    let Some(to) = &config.mail_to else { return Ok(()) };
    let mail = Message::builder()
        .from(config.mail_from.parse().context("monitor.mail_from")?)
        .to(to.parse().context("monitor.mail_to")?)
        .subject("Tool monitor alert")
        .header(ContentType::TEXT_HTML)
        .body(message.to_string())?;
    // The Toolforge mail relay, inside the cloud: no TLS.
    AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&config.smtp_host).build().send(mail).await?;
    Ok(())
}

async fn json(client: &reqwest::Client, url: &str) -> Option<Value> {
    client.get(url).send().await.ok()?.error_for_status().ok()?.json().await.ok()
}

/// `20240102030405` as a time.
fn timestamp(ts: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(ts.get(0..14)?, "%Y%m%d%H%M%S").ok()
}

fn older_than(time: NaiveDateTime, minutes: i64) -> bool {
    Utc::now().naive_utc() - time > chrono::Duration::minutes(minutes)
}

async fn quickstatements(client: &reqwest::Client, alerts: &mut Alerts) {
    let alerts = alerts.tool("QuickStatements");
    let Some(json) = json(client, "https://quickstatements.toolforge.org/api.php?action=get_batches_info").await else {
        return alerts.push("API does not answer".to_string());
    };
    // Just created doesn't count as activity.
    let latest = json["data"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(_, b)| &b["batch"])
        .filter(|b| b["ts_created"] != b["ts_last_change"])
        .filter_map(|b| timestamp(b["ts_last_change"].as_str()?))
        .max();
    match latest {
        Some(t) if older_than(t, 60) => alerts.push(format!("Last batch edit was more than 60 minutes ago ({t})")),
        None => alerts.push("No batch activity found".to_string()),
        _ => {}
    }
}

/// The PHP version only checked that a URL could be built.
async fn petscan(client: &reqwest::Client, alerts: &mut Alerts) {
    let alerts = alerts.tool("PetScan");
    let ok = client.get("https://petscan.wmflabs.org/").send().await.is_ok_and(|r| r.status().is_success());
    if !ok {
        alerts.push("Website does not answer".to_string());
    }
}

async fn mixnmatch(client: &reqwest::Client, alerts: &mut Alerts) {
    let alerts = alerts.tool("Mix'n'match");
    let Some(json) = json(client, "https://mix-n-match.toolforge.org/api.php?query=get_jobs&max=50").await else {
        return alerts.push("Can't get job info from API".to_string());
    };
    let latest = json["data"].as_array().into_iter().flatten().filter_map(|j| timestamp(j["last_ts"].as_str()?)).max();
    match latest {
        Some(t) if older_than(t, 60) => alerts.push(format!("Last job change was more than 60 minutes ago ({t})")),
        None => alerts.push("Can't get job info from API".to_string()),
        _ => {}
    }
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

async fn listeriabot(client: &reqwest::Client, alerts: &mut Alerts) {
    let alerts = alerts.tool("ListeriaBot");
    let url =
        "https://en.wikipedia.org/w/api.php?action=query&list=usercontribs&ucuser=ListeriaBot&uclimit=1&format=json";
    let last = json(client, url).await.and_then(|j| {
        let t = j["query"]["usercontribs"][0]["timestamp"].as_str()?.to_string();
        DateTime::parse_from_rfc3339(&t).ok()
    });
    match last {
        Some(t) if older_than(t.naive_utc(), 8 * 60) => {
            alerts.push("Last enwiki edit was more than 8 hours ago".to_string())
        }
        None => alerts.push("Can't get user contributions from enwiki API".to_string()),
        _ => {}
    }
}

/// Tools whose web address answered with something other than 200, by the `webstatus` job.
async fn web_status(state: &AppState, alerts: &mut Alerts) -> Result<()> {
    let mut conn = state.tool_db.conn().await?;
    let broken: Vec<String> = conn
        .query(
            "SELECT DISTINCT subdomain FROM tool WHERE has_webservice=1 AND (http_status!=200 OR http_status IS NULL)",
        )
        .await?;
    for subdomain in broken.iter().filter(|s| !OWN_CHECKS.contains(&s.as_str())) {
        if !alerts.has(subdomain) {
            alerts.tool(subdomain).push("Buggregator reports non-200 status".to_string());
        }
    }
    Ok(())
}

/// Services on the dashboard. Messages stay the same while the situation
/// does, or every run would mail.
async fn dashboard_services(state: &AppState, alerts: &mut Alerts) -> Result<()> {
    let mut conn = state.tool_db.conn().await?;
    let overview = query::overview(&mut conn, dashboard::now()).await?;
    for service in overview.as_array().into_iter().flatten() {
        for (component, c) in service["components"].as_object().into_iter().flatten() {
            let label = service["label"].as_str().unwrap_or_default();
            let alerts = alerts.tool(&format!("{label} ({component})"));
            let last_seen = c["current"]["last_seen"]
                .as_i64()
                .and_then(|t| DateTime::from_timestamp(t, 0))
                .map_or_else(String::new, |t| t.format("%Y-%m-%d %H:%M").to_string());
            match c["status"].as_str() {
                Some("down") => alerts.push(format!("No report since {last_seen}")),
                Some("stopped") => {
                    let reason = c["current"]["end_reason"].as_str().unwrap_or_default();
                    alerts.push(format!("Stopped ({reason}) at {last_seen}"));
                }
                _ => {}
            }
            let unplanned: u64 = c["restarts_1h"]
                .as_object()
                .into_iter()
                .flatten()
                .filter(|(reason, _)| !EXPECTED_END_REASONS.contains(&reason.as_str()))
                .filter_map(|(_, n)| n.as_u64())
                .sum();
            if unplanned >= 3 {
                alerts.push("Crash loop: 3 or more unplanned restarts within an hour".to_string());
            }
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

    #[test]
    fn timestamps() {
        assert_eq!(timestamp("20240102030405").map(|t| t.to_string()).as_deref(), Some("2024-01-02 03:04:05"));
        assert!(timestamp("2024").is_none());
    }
}
