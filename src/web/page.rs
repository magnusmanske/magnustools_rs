//! The look shared by all pages: `templates/base.html` and its stylesheet.

use axum::http::header;
use axum::response::IntoResponse;

/// Where `templates/base.html` loads the stylesheet from.
pub const STYLESHEET_PATH: &str = "/assets/style.css";
const STYLESHEET: &str = include_str!("../../assets/style.css");

pub async fn stylesheet() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/css; charset=utf-8"), (header::CACHE_CONTROL, "public, max-age=3600")], STYLESHEET)
}
