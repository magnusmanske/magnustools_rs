//! `buggregator/ingest.php`: receives the pushes of `wikimisc`'s telemetry,
//! `POST` with `Authorization: Bearer <token>` (or `X-Dashboard-Token`).

use super::dashboard::{self, ingest, payload::Push};
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::response::{IntoResponse, Json, Response};
use serde_json::json;
use std::sync::Arc;
use wikimisc::telemetry_wire::MAX_BODY_BYTES;

pub async fn handler(State(state): State<Arc<AppState>>, method: Method, headers: HeaderMap, body: Bytes) -> Response {
    if method != Method::POST {
        return respond(StatusCode::METHOD_NOT_ALLOWED, "POST required");
    }
    if body.len() > MAX_BODY_BYTES {
        return respond(StatusCode::PAYLOAD_TOO_LARGE, "Payload too large");
    }
    match store(&state, &headers, &body).await {
        Ok(response) => response,
        Err(e) => {
            tracing::error!("ingest: {e:#}");
            respond(StatusCode::INTERNAL_SERVER_ERROR, "Internal error")
        }
    }
}

async fn store(state: &AppState, headers: &HeaderMap, body: &[u8]) -> anyhow::Result<Response> {
    let mut conn = state.tool_db.conn().await?;
    let Some(service_id) = ingest::service_for_token(&mut conn, &token(headers)).await? else {
        return Ok(respond(StatusCode::UNAUTHORIZED, "Invalid token"));
    };
    let now = dashboard::now();
    let push = match Push::parse(body, now) {
        Ok(push) => push,
        Err(message) => return Ok(respond(StatusCode::BAD_REQUEST, &message)),
    };
    Ok(match ingest::ingest(&mut conn, service_id, &push, now).await? {
        Ok(skipped) => Json(json!({"status": "OK", "skipped": skipped})).into_response(),
        Err(ingest::Rejected(code, message)) => {
            respond(StatusCode::from_u16(code).unwrap_or(StatusCode::BAD_REQUEST), message)
        }
    })
}

fn token(headers: &HeaderMap) -> String {
    let header = |name| headers.get(name).and_then(|v| v.to_str().ok()).unwrap_or_default();
    let authorization = header(header::AUTHORIZATION.as_str());
    match authorization.split_once(' ') {
        Some((scheme, token)) if scheme.eq_ignore_ascii_case("bearer") => token.trim().to_string(),
        _ => header("x-dashboard-token").to_string(),
    }
}

fn respond(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({"status": message}))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_from_headers() {
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, "Bearer abc".parse().unwrap());
        assert_eq!(token(&headers), "abc");
        let mut headers = HeaderMap::new();
        headers.insert("x-dashboard-token", "def".parse().unwrap());
        assert_eq!(token(&headers), "def");
        assert_eq!(token(&HeaderMap::new()), "");
    }
}
