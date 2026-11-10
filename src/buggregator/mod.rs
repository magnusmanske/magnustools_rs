//! Buggregator: issues from wiki talk pages and code forges in one place,
//! and the service dashboard. The front-end (`public_html/buggregator/`)
//! is served as static files; this is its backend.

mod api;
pub mod dashboard;
mod ingest;
pub mod issues;
mod toolinfo;
pub mod update;
mod usage;

use crate::state::AppState;
use crate::web::moved;
use axum::Router;
use axum::routing::{any, get};
use std::sync::Arc;

/// Where a tool's web interface should answer: the address of its dashboard
/// service if it has one (that is the one users see), else its own `url`,
/// else its Toolforge subdomain. `None` if there is nothing to check.
pub fn address(subdomain: &str, url: &str) -> Option<String> {
    let url = url.trim();
    if !url.is_empty() {
        Some(url.to_string())
    } else if !subdomain.trim().is_empty() {
        Some(format!("https://{}.toolforge.org", subdomain.trim()))
    } else {
        None
    }
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
}
