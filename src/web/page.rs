//! The look shared by all pages: `templates/base.html` and its stylesheet;
//! plus the JSONP responses of the APIs.

use super::AppError;
use askama::Template;
use axum::http::header;
use axum::response::{Html, IntoResponse};
use serde::Serialize;
use wikimisc::server::jsonp::{CallbackRules, Jsonp};

/// Where `templates/base.html` loads the stylesheet from.
pub const STYLESHEET_PATH: &str = "/assets/style.css";
const STYLESHEET: &str = include_str!("../../assets/style.css");

pub async fn stylesheet() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/css; charset=utf-8"), (header::CACHE_CONTROL, "public, max-age=3600")], STYLESHEET)
}

pub fn render(template: &impl Template) -> Result<Html<String>, AppError> {
    template.render().map(Html).map_err(|e| AppError::Internal(format!("template: {e}")))
}

/// `value` as JSON, or as JSONP if there is a callback. Callers of the old
/// tools use jQuery-style names (`jQuery123_456`) and dotted paths.
pub fn jsonp<T: Serialize>(value: T, callback: Option<String>) -> Jsonp<T> {
    Jsonp::new(value, callback).rules(CallbackRules { allow_dotted: true, max_len: 128 })
}

pub use wikimisc::format::thousands;

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;

    async fn body_of(response: axum::response::Response) -> (StatusCode, String) {
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    #[test]
    fn thousands_separators() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1000), "1,000");
        assert_eq!(thousands(1234567), "1,234,567");
    }

    #[tokio::test]
    async fn jsonp_accepts_dotted_callbacks_and_refuses_code() {
        let (status, body) =
            body_of(jsonp(serde_json::json!({"a": 1}), Some("jQuery123_456.cb".into())).into_response()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "jQuery123_456.cb({\"a\":1});");

        for callback in ["a-b", "a..b", "alert(1)", "1abc"] {
            let (status, _) = body_of(jsonp(serde_json::json!(1), Some(callback.into())).into_response()).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{callback}");
        }
        // An empty callback means plain JSON.
        let (status, body) = body_of(jsonp(serde_json::json!(1), Some(String::new())).into_response()).await;
        assert_eq!((status, body.as_str()), (StatusCode::OK, "1"));
    }

    #[tokio::test]
    async fn jsonp_callbacks_are_length_limited() {
        let callback = "a".repeat(129);
        let (status, _) = body_of(jsonp(serde_json::json!(1), Some(callback)).into_response()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
}
