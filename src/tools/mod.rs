//! The tools, one module each, at the URLs of their PHP originals.

mod html2wiki;
mod tab2wiki;

use crate::state::AppState;
use axum::Router;
use axum::routing::get;
use std::sync::Arc;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/html2wiki.php", get(html2wiki::handler).post(html2wiki::handler))
        .route("/tab2wiki.php", get(tab2wiki::handler).post(tab2wiki::handler))
}
