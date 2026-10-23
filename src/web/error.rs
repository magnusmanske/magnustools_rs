//! Errors a request can end in, shown as themed pages.

use crate::wiki::UnknownWiki;
use askama::Template;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// The message is shown to the user.
    #[error("{0}")]
    BadRequest(String),
    #[error("not found")]
    NotFound,
    /// A retired page.
    #[error("gone")]
    Gone,
    /// A wiki or other service failed us. Details are logged, not shown.
    #[error(transparent)]
    Upstream(#[from] anyhow::Error),
    #[error("{0}")]
    Internal(String),
}

impl From<UnknownWiki> for AppError {
    fn from(e: UnknownWiki) -> Self {
        Self::BadRequest(e.to_string())
    }
}

#[derive(Template)]
#[template(path = "error.html")]
struct ErrorPage<'a> {
    title: &'a str,
    message: &'a str,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            Self::BadRequest(message) => (StatusCode::BAD_REQUEST, message.as_str()),
            Self::NotFound => (StatusCode::NOT_FOUND, "There is nothing at this address."),
            Self::Gone => (StatusCode::GONE, "This page has been retired."),
            Self::Upstream(e) => {
                tracing::warn!("upstream failure: {e:#}");
                (
                    StatusCode::BAD_GATEWAY,
                    "A server this tool relies on did not answer as expected. Please try again later.",
                )
            }
            Self::Internal(e) => {
                tracing::error!("internal error: {e}");
                (StatusCode::INTERNAL_SERVER_ERROR, "Something went wrong on our side.")
            }
        };
        let title = status.canonical_reason().unwrap_or("Error");
        match (ErrorPage { title, message }).render() {
            Ok(html) => (status, Html(html)).into_response(),
            Err(_) => (status, message.to_string()).into_response(),
        }
    }
}
