//! HTTP layer: the tool's pages, the PHP tool's static files, and what
//! became of retired URLs.

mod error;
mod legacy;
mod page;
mod static_files;

pub use error::AppError;

use crate::config::Config;
use axum::Router;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::get;
use std::sync::Arc;
use std::time::Duration;
use tower_http::set_header::SetResponseHeaderLayer;

/// Some replica queries legitimately take minutes.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);
const HEALTH_PATH: &str = "/healthz";

pub fn router(config: Arc<Config>) -> Router {
    let router = legacy::routes()
        .route(HEALTH_PATH, get(wikimisc::server::health::ok))
        .route(page::STYLESHEET_PATH, get(page::stylesheet))
        .fallback_service(static_files::service(&config.public_html))
        .layer(header_layer(header::X_CONTENT_TYPE_OPTIONS, "nosniff"))
        .layer(header_layer(header::REFERRER_POLICY, "strict-origin-when-cross-origin"));
    let router = wikimisc::server::timeout(router, REQUEST_TIMEOUT, StatusCode::GATEWAY_TIMEOUT);
    let router = wikimisc::server::standard_layers(router);
    // Each request's status and duration, for the dashboard.
    wikimisc::server::request_metrics(router, &[HEALTH_PATH])
}

fn header_layer(name: header::HeaderName, value: &'static str) -> SetResponseHeaderLayer<HeaderValue> {
    SetResponseHeaderLayer::if_not_present(name, HeaderValue::from_static(value))
}

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
        let config = Config { public_html: public_html.to_path_buf(), telemetry: Default::default() };
        let request = Request::get(uri).body(Body::empty()).unwrap();
        let response = router(Arc::new(config)).oneshot(request).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
        TestResponse { status, headers, body: String::from_utf8_lossy(&bytes).into_owned() }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::get;

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
    async fn unknown_paths_get_the_themed_404() {
        let dir = tempfile::tempdir().unwrap();
        let response = get(dir.path(), "/no_such_tool.php").await;
        assert_eq!(response.status.as_u16(), 404);
        assert!(response.body.contains("/assets/style.css"));
    }
}
