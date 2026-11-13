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

impl From<crate::mediawiki::MediaWikiError> for AppError {
    fn from(e: crate::mediawiki::MediaWikiError) -> Self {
        match e {
            crate::mediawiki::MediaWikiError::Missing(title) => {
                Self::BadRequest(format!("The page “{}” does not exist.", title.pretty()))
            }
            e => Self::Upstream(anyhow::Error::new(e)),
        }
    }
}

impl From<crate::mediawiki::WikiApiError> for AppError {
    fn from(e: crate::mediawiki::WikiApiError) -> Self {
        Self::Upstream(anyhow::Error::new(e))
    }
}

impl From<crate::replicas::ReplicasError> for AppError {
    fn from(e: crate::replicas::ReplicasError) -> Self {
        Self::Upstream(anyhow::Error::new(e))
    }
}

impl From<crate::oauth::OAuthError> for AppError {
    fn from(e: crate::oauth::OAuthError) -> Self {
        Self::Upstream(anyhow::Error::new(e))
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

#[cfg(test)]
mod tests {
    use super::*;

    async fn parts(error: AppError) -> (StatusCode, String) {
        let response = error.into_response();
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    #[tokio::test]
    async fn each_kind_has_its_status_and_message() {
        let (status, body) = parts(AppError::BadRequest("bad <input>".into())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("bad &#60;input&#62;"), "{body}");

        let (status, body) = parts(AppError::NotFound).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body.contains("nothing at this address"));

        let (status, body) = parts(AppError::Gone).await;
        assert_eq!(status, StatusCode::GONE);
        assert!(body.contains("has been retired"));
    }

    #[tokio::test]
    async fn internals_are_logged_but_not_shown() {
        for error in [
            AppError::Upstream(anyhow::anyhow!("database password in here")),
            AppError::Internal("secret path /etc/passwd".into()),
        ] {
            let (status, body) = parts(error).await;
            assert!(body.contains("Something went wrong") || body.contains("did not answer as expected"), "{body}");
            assert!(!body.contains("password") && !body.contains("passwd"), "{body}");
            assert!(status == StatusCode::BAD_GATEWAY || status == StatusCode::INTERNAL_SERVER_ERROR);
        }
    }

    #[test]
    fn unknown_wikis_are_bad_requests() {
        let error = AppError::from(UnknownWiki { language: "xx".into(), project: "evil".into() });
        assert!(matches!(error, AppError::BadRequest(ref message) if message.contains("xx.evil")));
    }

    #[test]
    fn a_missing_page_is_a_bad_request_with_its_title() {
        let title = crate::mediawiki::Title::new("File:Some file.jpg", 6);
        let error = AppError::from(crate::mediawiki::MediaWikiError::Missing(title));
        assert!(matches!(error, AppError::BadRequest(ref message) if message.contains("Some file.jpg")));
    }
}
