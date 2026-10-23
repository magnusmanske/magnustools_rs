//! Buggregator: issues from wiki talk pages and code forges in one place,
//! and the service dashboard. The front-end (`public_html/buggregator/`)
//! is served as static files; this is its backend.

mod api;
pub mod dashboard;
mod ingest;
pub mod issues;
mod toolinfo;
pub mod update;

use crate::state::AppState;
use axum::Router;
use axum::routing::{any, get};
use std::sync::Arc;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/buggregator/api.php", get(api::handler).post(api::handler))
        .route("/buggregator/ingest.php", any(ingest::handler))
        .route("/toolinfo_buggregator.json", get(toolinfo::handler))
}
