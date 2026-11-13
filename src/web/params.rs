//! Request parameters the way the old PHP tools read them (`$_REQUEST`):
//! from the query string and, for form posts, the body. Forms keep working
//! whether they use GET or POST, and links to results keep working too.

use super::AppError;
use axum::body::Bytes;
use axum::extract::{FromRequest, Request};
use axum::http::header;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};

/// Deserializes `T` from query and form parameters; on a repeated name the
/// last value wins, so the body beats the query string, as in PHP.
#[derive(Debug, Clone)]
pub struct Params<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequest<S> for Params<T> {
    type Rejection = AppError;

    async fn from_request(request: Request, state: &S) -> Result<Self, AppError> {
        let query = request.uri().query().unwrap_or_default().to_string();
        let is_form = request
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("application/x-www-form-urlencoded"));
        let body = match is_form {
            true => Bytes::from_request(request, state).await.map_err(|e| AppError::BadRequest(e.body_text()))?,
            false => Bytes::new(),
        };
        merge(&query, &body).map(Params)
    }
}

fn merge<T: DeserializeOwned>(query: &str, body: &[u8]) -> Result<T, AppError> {
    wikimisc::web::params::merge(query, body).map_err(|e| AppError::BadRequest(e.to_string()))
}

/// A PHP checkbox: any value but the empty string or `"0"` is on, and an
/// absent parameter is off. Use with `#[serde(default, deserialize_with = "php_bool")]`.
pub fn php_bool<'de, D: Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    let value = Option::<String>::deserialize(deserializer)?;
    Ok(value.is_some_and(|value| !value.is_empty() && value != "0"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Deserialize, PartialEq)]
    struct P {
        #[serde(default)]
        text: String,
        depth: Option<u32>,
        doit: Option<String>,
    }

    #[test]
    fn query_and_body_merge_with_the_body_winning() {
        let p: P = merge("text=from+query&depth=3", b"text=from%20body&doit=1").unwrap();
        assert_eq!(p, P { text: "from body".into(), depth: Some(3), doit: Some("1".into()) });
    }

    #[test]
    fn missing_and_invalid() {
        let p: P = merge("", b"").unwrap();
        assert_eq!(p, P { text: String::new(), depth: None, doit: None });
        assert!(merge::<P>("depth=deep", b"").is_err());
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct Flags {
        #[serde(default, deserialize_with = "php_bool")]
        on: bool,
    }

    #[test]
    fn php_checkboxes_are_booleans() {
        for (query, expected) in [("", false), ("on=", false), ("on=0", false), ("on=1", true), ("on=yes", true)] {
            assert_eq!(merge::<Flags>(query, b"").unwrap(), Flags { on: expected }, "{query}");
        }
    }
}
