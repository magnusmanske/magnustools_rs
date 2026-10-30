//! HTTP layer: the tool's pages, the PHP tool's static files, and what
//! became of retired URLs.

mod apps;
mod error;
mod legacy;
mod page;
mod params;
mod static_files;

pub use error::AppError;
pub use page::{jsonp, render, thousands};
pub use params::Params;

use crate::state::AppState;
use axum::Router;
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use std::sync::Arc;
use std::time::Duration;
use tower_http::set_header::SetResponseHeaderLayer;

/// Some replica queries legitimately take minutes.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);
const HEALTH_PATH: &str = "/healthz";
/// Not counted as traffic: health checks, and the logger, which the tools
/// themselves call (under its old address too).
const UNCOUNTED_PATHS: &[&str] = &[HEALTH_PATH, "/logger/", "/logger.php", "/logger"];

pub fn router(state: Arc<AppState>) -> Router {
    let static_files = static_files::service(state.config.public_html.as_deref());
    // Fails while the database pool is stuck, so the pod is restarted.
    let health = {
        let state = state.clone();
        wikimisc::server::health::readiness(move || !state.tool_db.is_stuck(), "tool database connections are stuck")
    };
    let router = crate::tools::routes()
        .merge(crate::buggregator::routes())
        .route(HEALTH_PATH, health)
        .route(page::STYLESHEET_PATH, get(page::stylesheet))
        .route("/tool_status.html", get(tool_status))
        .with_state(state)
        .merge(legacy::routes())
        .fallback_service(static_files)
        .layer(header_layer(header::X_CONTENT_TYPE_OPTIONS, "nosniff"))
        .layer(header_layer(header::REFERRER_POLICY, "strict-origin-when-cross-origin"));
    let router = wikimisc::server::timeout(router, REQUEST_TIMEOUT, StatusCode::GATEWAY_TIMEOUT);
    let router = wikimisc::server::standard_layers(router);
    // Each request's status and duration, for the dashboard.
    wikimisc::server::request_metrics(router, UNCOUNTED_PATHS)
}

/// The latest page of the `monitor` job.
async fn tool_status(State(state): State<Arc<AppState>>) -> Response {
    match tokio::fs::read_to_string(&state.config.monitor.status_page).await {
        Ok(html) => Html(html).into_response(),
        Err(_) => AppError::NotFound.into_response(),
    }
}

fn header_layer(name: header::HeaderName, value: &'static str) -> SetResponseHeaderLayer<HeaderValue> {
    SetResponseHeaderLayer::if_not_present(name, HeaderValue::from_static(value))
}

pub use wikimisc::server::{is_same_origin, moved};

/// The themed 404 page, for anything no route or file matches.
async fn not_found() -> impl IntoResponse {
    AppError::NotFound
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    pub struct TestResponse {
        pub status: StatusCode,
        pub headers: axum::http::HeaderMap,
        pub body: String,
    }

    /// The full router, serving static files from `public_html`.
    pub async fn get(public_html: &std::path::Path, uri: &str) -> TestResponse {
        // Credentials are only read here; nothing connects.
        let credentials = public_html.join("test.my.cnf");
        std::fs::write(&credentials, "[client]\nuser=test\npassword=test\n").unwrap();
        let config: crate::config::Config = serde_json::from_value(serde_json::json!({
            "public_html": public_html,
            "monitor": { "status_page": public_html.join("status.html") },
            "replica_credentials": credentials,
            "buggregator_oauth": public_html.join("none.ini"),
            "uploader_oauth": public_html.join("none.ini"),
        }))
        .unwrap();
        let state = AppState::new(config).unwrap();
        let request = Request::get(uri).body(Body::empty()).unwrap();
        let response = router(Arc::new(state)).oneshot(request).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
        TestResponse { status, headers, body: String::from_utf8_lossy(&bytes).into_owned() }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::get;
    use super::*;
    use axum::http::HeaderMap;

    #[test]
    fn same_origin() {
        let headers = |pairs: &[(&'static str, &str)]| {
            let mut h = HeaderMap::new();
            for (k, v) in pairs {
                h.insert(*k, v.parse().unwrap());
            }
            h
        };
        assert!(is_same_origin(&headers(&[("sec-fetch-site", "same-origin")])));
        assert!(!is_same_origin(&headers(&[
            ("sec-fetch-site", "cross-site"),
            ("host", "a.org"),
            ("referer", "https://a.org/")
        ])));
        assert!(is_same_origin(&headers(&[("host", "a.org"), ("referer", "https://a.org/buggregator/")])));
        assert!(!is_same_origin(&headers(&[("host", "a.org"), ("origin", "https://a.org.evil.com")])));
        assert!(!is_same_origin(&headers(&[("host", "a.org")])));
    }

    #[tokio::test]
    async fn health_and_stylesheet() {
        let dir = tempfile::tempdir().unwrap();
        let health = get(dir.path(), "/healthz").await;
        assert_eq!((health.status.as_u16(), health.body.as_str()), (200, "ok"));
        let css = get(dir.path(), "/assets/style.css").await;
        assert_eq!(css.status.as_u16(), 200);
        assert_eq!(css.headers["content-type"], "text/css; charset=utf-8");
        assert_eq!(css.headers["x-content-type-options"], "nosniff");
    }

    #[tokio::test]
    async fn the_monitors_status_page() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(get(dir.path(), "/tool_status.html").await.status.as_u16(), 404);
        std::fs::write(dir.path().join("status.html"), "<p>all well</p>").unwrap();
        assert_eq!(get(dir.path(), "/tool_status.html").await.body, "<p>all well</p>");
    }

    #[tokio::test]
    async fn unknown_paths_get_the_themed_404() {
        let dir = tempfile::tempdir().unwrap();
        let response = get(dir.path(), "/no_such_tool.php").await;
        assert_eq!(response.status.as_u16(), 404);
        assert!(response.body.contains("/assets/style.css"));
    }
}
