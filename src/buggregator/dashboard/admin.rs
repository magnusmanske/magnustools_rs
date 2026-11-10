//! Service registration, token handling, and alert acknowledgements.

use super::ingest::hash_token;
use anyhow::{Result, bail};
use wikimisc::mysql_async::Conn;
use wikimisc::mysql_async::prelude::Queryable;

pub const ACK_KINDS: &[&str] = &["status", "restarts", "memory"];

/// Hides a dashboard alert until whatever triggers it happens again. The
/// error is for the user.
pub async fn acknowledge(
    conn: &mut Conn,
    service: &str,
    component: &str,
    kind: &str,
    user: &str,
    now: i64,
) -> Result<Result<(), String>> {
    if !ACK_KINDS.contains(&kind) {
        return Ok(Err(format!("Invalid alert kind '{kind}'")));
    }
    let service_id: Option<u64> = conn
        .exec_first(
            "SELECT DISTINCT service_id FROM dash_instance
             WHERE service_id=(SELECT id FROM dash_service WHERE name=?) AND component=?",
            (service, component),
        )
        .await?;
    let Some(service_id) = service_id else {
        return Ok(Err(format!("No component '{component}' of service '{service}'")));
    };
    conn.exec_drop(
        "INSERT INTO dash_ack (service_id, component, kind, acked_at, acked_by) VALUES (?, ?, ?, ?, ?)
         ON DUPLICATE KEY UPDATE acked_at=VALUES(acked_at), acked_by=VALUES(acked_by)",
        (service_id, component, kind, now, user),
    )
    .await?;
    Ok(Ok(()))
}

/// Registers a service; returns its token, which is not stored and cannot be shown again.
pub async fn add_service(
    conn: &mut Conn,
    name: &str,
    label: &str,
    url: &str,
    tool_id: Option<u64>,
    now: i64,
) -> Result<String> {
    let valid = name.len() <= 64
        && name.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
    if !valid {
        bail!("Invalid service name '{name}'");
    }
    let url = if url.trim().is_empty() {
        String::new()
    } else {
        crate::buggregator::safe_url(url).ok_or_else(|| anyhow::anyhow!("Invalid service URL '{url}'"))?
    };
    let token = new_token();
    conn.exec_drop(
        "INSERT INTO dash_service (name, label, url, tool_id, token_hash, created_at) VALUES (?, ?, ?, ?, ?, ?)",
        (name, label, url.as_str(), tool_id, hash_token(&token), now),
    )
    .await?;
    Ok(token)
}

/// Replaces a service's token; the old one stops working at once.
pub async fn rotate_token(conn: &mut Conn, name: &str) -> Result<String> {
    let token = new_token();
    conn.exec_drop("UPDATE dash_service SET token_hash=? WHERE name=?", (hash_token(&token), name)).await?;
    if conn.affected_rows() == 0 {
        bail!("No service '{name}'");
    }
    Ok(token)
}

pub async fn set_active(conn: &mut Conn, name: &str, active: bool) -> Result<()> {
    conn.exec_drop("UPDATE dash_service SET is_active=? WHERE name=?", (active, name)).await?;
    Ok(())
}

/// `(name, active, tool id, label)` of all services.
pub async fn list_services(conn: &mut Conn) -> Result<Vec<(String, bool, Option<u64>, String)>> {
    Ok(conn.query("SELECT name, is_active, tool_id, label FROM dash_service ORDER BY name").await?)
}

fn new_token() -> String {
    rand::random::<[u8; 32]>().iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens() {
        let token = new_token();
        assert_eq!(token.len(), 64);
        assert!(token.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_ne!(token, new_token());
    }
}
