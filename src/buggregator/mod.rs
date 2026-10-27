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
