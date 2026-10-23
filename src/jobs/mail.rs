//! Alert mails, through the Toolforge mail relay.

use crate::config::MonitorConfig;
use anyhow::{Context, Result};
use lettre::message::header::ContentType;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

/// Sends an HTML mail to `monitor.mail_to`; without one, does nothing.
pub async fn send(config: &MonitorConfig, subject: &str, html: &str) -> Result<()> {
    let Some(to) = &config.mail_to else { return Ok(()) };
    let mail = Message::builder()
        .from(config.mail_from.parse().context("monitor.mail_from")?)
        .to(to.parse().context("monitor.mail_to")?)
        .subject(subject)
        .header(ContentType::TEXT_HTML)
        .body(html.to_string())?;
    // The Toolforge mail relay, inside the cloud: no TLS.
    AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&config.smtp_host).build().send(mail).await?;
    Ok(())
}
