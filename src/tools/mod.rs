//! The tools, one module each, at the URLs of their PHP originals.

mod cas;
mod get_article_intro;
mod html2wiki;
mod spacer;
mod tab2wiki;

use crate::state::AppState;
use axum::Router;
use axum::routing::get;
use std::sync::Arc;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/cas.php", get(cas::handler))
        .route("/get_article_intro.php", get(get_article_intro::handler))
        .route("/html2wiki.php", get(html2wiki::handler).post(html2wiki::handler))
        .route("/spacer.php", get(spacer::handler))
        .route("/tab2wiki.php", get(tab2wiki::handler).post(tab2wiki::handler))
}
