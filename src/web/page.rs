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

/// `1234567` as `1,234,567`.
pub fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thousands_separators() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1000), "1,000");
        assert_eq!(thousands(1234567), "1,234,567");
    }
}
