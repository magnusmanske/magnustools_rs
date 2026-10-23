//! Errors a request can end in, shown as themed pages.

use askama::Template;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("not found")]
    NotFound,
    /// A retired page.
    #[error("gone")]
    Gone,
}

#[derive(Template)]
#[template(path = "error.html")]
struct ErrorPage<'a> {
    title: &'a str,
    message: &'a str,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::NotFound => (StatusCode::NOT_FOUND, "There is nothing at this address."),
            Self::Gone => (StatusCode::GONE, "This page has been retired."),
        };
        let title = status.canonical_reason().unwrap_or("Error");
        match (ErrorPage { title, message }).render() {
            Ok(html) => (status, Html(html)).into_response(),
            Err(_) => (status, message).into_response(),
        }
    }
}
