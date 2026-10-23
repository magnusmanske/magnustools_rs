//! `authority_control.php`: a JSON(P) proxy for VIAF, by name (`key`) or
//! cluster ID (`id`), for scripts that cannot call VIAF directly.

use crate::state::AppState;
use crate::web::{Params, jsonp};
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;

const VIAF_SEARCH: &str = "https://viaf.org/viaf/search";
const VIAF_CLUSTER: &str = "https://viaf.org/api/cluster-record";

#[derive(Debug, Default, Deserialize)]
pub struct Request {
    query: Option<String>,
    #[serde(default)]
    key: String,
    #[serde(default)]
    id: String,
    callback: Option<String>,
}

#[derive(Debug, Serialize)]
struct Answer {
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
}

impl Answer {
    fn status(status: &str) -> Self {
        Self { status: status.to_string(), result: None }
    }
}

pub async fn handler(State(state): State<Arc<AppState>>, Params(request): Params<Request>) -> Response {
    let answer = match request.query.as_deref() {
        Some("viaf") => viaf(&state.http, request.key.trim(), request.id.trim()).await,
        // The old GND lookup never returned anything.
        Some("gnd") => Answer::status("OK"),
        _ => Answer::status("query must be 'viaf' or 'gnd'"),
    };
    jsonp(answer, request.callback).into_response()
}

async fn viaf(http: &reqwest::Client, key: &str, id: &str) -> Answer {
    let request = if !key.is_empty() {
        http.get(VIAF_SEARCH)
            .query(&[("query", format!("local.names = {key}").as_str()), ("maximumRecords", "10")])
            .header(reqwest::header::ACCEPT, "application/json")
    } else if !id.is_empty() {
        let payload = json!({
            "reqValues": {"recordId": id, "isSourceId": false},
            "meta": {"env": "prod", "pageIndex": 0, "pageSize": 1},
        });
        http.post(VIAF_CLUSTER).json(&payload)
    } else {
        return Answer::status("key or id required");
    };
    let result = match request.send().await {
        Ok(response) => response.json::<Value>().await.ok(),
        Err(e) => {
            tracing::warn!("VIAF: {e}");
            None
        }
    };
    match result {
        Some(result) => Answer { status: "OK".to_string(), result: Some(result) },
        None => Answer { status: "VIAF did not answer".to_string(), result: Some(Value::Null) },
    }
}
