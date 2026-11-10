//! Buggregator: issues from wiki talk pages and code forges in one place,
//! and the service dashboard. The front-end (`public_html/buggregator/`)
//! is served as static files; this is its backend.

mod api;
pub mod dashboard;
pub(crate) mod dedupe;
mod ingest;
pub mod issues;
mod toolinfo;
pub mod update;
mod usage;

use crate::state::AppState;
use crate::web::moved;
use anyhow::Result;
use axum::Router;
use axum::routing::{any, get};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use wikimisc::mysql_async::prelude::Queryable;

/// Lower-case SHA-256, as the token and text columns store it.
pub fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

/// Who maintains the tool; also the user a maintenance action logs as.
pub const MAINTAINER: &str = "Magnus Manske";

/// An absolute HTTP(S) URL, trimmed; `None` for anything a link must not use
/// (e.g. `javascript:`). Empty input is `None`.
pub fn safe_url(url: &str) -> Option<String> {
    let url = url.trim();
    let lower = url.to_ascii_lowercase();
    (lower.starts_with("http://") || lower.starts_with("https://")).then(|| url.to_string())
}

/// The auto-increment id of the row just written on `conn` (`LAST_INSERT_ID()`).
pub async fn last_id(conn: &mut impl Queryable) -> Result<u64> {
    conn.query_first::<u64, _>("SELECT LAST_INSERT_ID()").await?.ok_or_else(|| anyhow::anyhow!("no insert id"))
}

/// Where a tool's web interface should answer: the address of its dashboard
/// service if it has one (that is the one users see), else its own `url`,
/// else its Toolforge subdomain. `None` if there is nothing to check.
pub fn address(subdomain: &str, url: &str) -> Option<String> {
    if let Some(url) = safe_url(url) {
        return Some(url);
    }
    if !subdomain.trim().is_empty() { Some(format!("https://{}.toolforge.org", subdomain.trim())) } else { None }
}

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/buggregator/api/", get(api::handler).post(api::handler))
        .route("/buggregator/api.php", moved("/buggregator/api/"))
        .route("/buggregator/api", moved("/buggregator/api/"))
        .route("/buggregator/ingest/", any(ingest::handler))
        // Not a redirect: wikimisc's telemetry client pushes here, and does not follow redirects.
        .route("/buggregator/ingest.php", any(ingest::handler))
        .route("/toolinfo_buggregator.json", get(toolinfo::handler))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tool_answers_at_its_own_address() {
        assert_eq!(address("petscan", "https://petscan.wmcloud.org/").as_deref(), Some("https://petscan.wmcloud.org/"));
        assert_eq!(address("petscan", "  ").as_deref(), Some("https://petscan.toolforge.org"));
        assert_eq!(address("", "").as_deref(), None);
    }

    #[test]
    fn only_http_urls_become_links() {
        assert_eq!(safe_url(" https://x.org/a ").as_deref(), Some("https://x.org/a"));
        assert_eq!(safe_url("HTTP://x.org").as_deref(), Some("HTTP://x.org"));
        assert_eq!(safe_url("javascript:alert(1)"), None);
        assert_eq!(safe_url("//x.org"), None);
        assert_eq!(safe_url(""), None);
        // An unsafe URL does not hide the Toolforge address.
        assert_eq!(address("petscan", "javascript:x").as_deref(), Some("https://petscan.toolforge.org"));
    }
}
