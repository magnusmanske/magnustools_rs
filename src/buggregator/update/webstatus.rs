//! What each tool's web address answers, for the status page and the alerts.

use anyhow::Result;
use chrono::Utc;
use std::collections::BTreeMap;
use wikimisc::mysql_async::Conn;
use wikimisc::mysql_async::prelude::Queryable;

/// Tries per tool: the Toolforge proxy now and then answers 503 for a
/// connection it dropped, though the tool is fine.
const STATUS_ATTEMPTS: usize = 3;
const STATUS_RETRY_AFTER: std::time::Duration = std::time::Duration::from_secs(10);
/// Addresses checked at once; the job should finish well within its window.
const WEB_STATUS_CONCURRENT: usize = 8;
/// Some tools legitimately take tens of seconds (large replica queries); a
/// shorter timeout would mark a working tool as not answering.
const WEB_STATUS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// A `tool` row for the address check: id, active, web interface, subdomain,
/// URL (its own or its service's; `None` when neither is set), service id,
/// status, and since.
type ToolRow = (u64, bool, bool, String, Option<String>, Option<u64>, Option<i64>, Option<i64>);
/// A tool answering at an address, with its status and since before this run.
type ToolStatus = (u64, Option<i64>, Option<i64>);

/// Sets each tool's `http_status` to what its web address answers, after
/// redirects (`0`: no answer, after a few tries), and `http_status_since` to
/// when that answer started, for the alerts. Tools that have no address, or
/// no web interface, get neither.
pub async fn web_status(conn: &mut Conn) -> Result<()> {
    let client = reqwest::Client::builder()
        .user_agent(crate::state::USER_AGENT)
        .redirect(reqwest::redirect::Policy::limited(5))
        .timeout(WEB_STATUS_TIMEOUT)
        .build()?;
    let tools: Vec<ToolRow> = conn
        .query(
            "SELECT t.id, t.is_active, t.has_webservice, t.subdomain,
                    COALESCE(NULLIF(s.url,''), NULLIF(t.url,'')), s.id, t.http_status, t.http_status_since
             FROM tool t LEFT JOIN dash_service s ON s.tool_id=t.id AND s.is_active=1",
        )
        .await?;
    // One request per address, however many tools answer there (Reasonator has several).
    let mut by_address: BTreeMap<String, Vec<ToolStatus>> = BTreeMap::new();
    let mut unchecked = vec![];
    for (id, is_active, has_webservice, subdomain, url, service, status, since) in tools {
        let url = url.unwrap_or_default();
        match crate::buggregator::address(&subdomain, &url) {
            Some(address) if is_active && (has_webservice || service.is_some()) => {
                by_address.entry(address).or_default().push((id, status, since));
            }
            _ => unchecked.push(id),
        }
    }
    for id in unchecked {
        conn.exec_drop("UPDATE tool SET http_status=NULL, http_status_since=NULL WHERE id=?", (id,)).await?;
    }
    let now = Utc::now().timestamp();
    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(WEB_STATUS_CONCURRENT));
    let mut tasks = tokio::task::JoinSet::new();
    for (address, tools) in by_address {
        let client = client.clone();
        let semaphore = semaphore.clone();
        tasks.spawn(async move {
            let Ok(_permit) = semaphore.acquire_owned().await else { return None };
            let status = web_answer(&client, &address).await;
            Some((address, tools, status))
        });
    }
    while let Some(result) = tasks.join_next().await {
        let Some((address, tools, status)) = result? else { continue };
        for (id, previous, previous_since) in tools {
            // An unchanged answer keeps its start; one that becomes fine has none.
            let since = if status == 200 {
                None
            } else if previous == Some(i64::from(status)) {
                previous_since
            } else {
                Some(now)
            };
            conn.exec_drop(
                "UPDATE tool SET http_status=?, http_status_since=? WHERE id=?",
                (i64::from(status), since, id),
            )
            .await?;
        }
        tracing::debug!("{address}: HTTP {status}");
    }
    Ok(())
}

/// What `url` answers, after redirects; `0` if it does not answer at all.
async fn web_answer(client: &reqwest::Client, url: &str) -> u16 {
    let mut status = 0;
    for attempt in 0..STATUS_ATTEMPTS {
        if attempt > 0 {
            tokio::time::sleep(STATUS_RETRY_AFTER).await;
        }
        status = answer_once(client, url).await;
        if status == 200 {
            break;
        }
    }
    status
}

/// HEAD first; some servers reject it, so ask for the first byte instead.
async fn answer_once(client: &reqwest::Client, url: &str) -> u16 {
    match client.head(url).send().await {
        Ok(response) if matches!(response.status().as_u16(), 405 | 501) => {
            let status = client
                .get(url)
                .header(reqwest::header::RANGE, "bytes=0-0")
                .send()
                .await
                .map_or(0, |r| r.status().as_u16());
            // A partial answer for the one requested byte means the address is fine.
            if status == 206 { 200 } else { status }
        }
        Ok(response) => response.status().as_u16(),
        Err(_) => 0,
    }
}
