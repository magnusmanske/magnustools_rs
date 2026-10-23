//! `logger.php`: counts uses of tools, per tool, method and day. Called by
//! many tools, from PHP and from JavaScript (as JSONP). The tools here are
//! counted directly, by [`count_uses`], as tool `magnustools`.

use crate::state::AppState;
use crate::web::{Params, jsonp};
use axum::extract::{Request as HttpRequest, State};
use axum::http::{HeaderValue, Method, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, LazyLock};
use wikimisc::mysql_async::prelude::Queryable;

/// The width of the name columns.
const MAX_NAME: usize = 128;
/// The log's tool name for the tools here; each is a method of it.
const THIS_TOOL: &str = "magnustools";

#[derive(Debug, Default, Deserialize)]
pub struct Request {
    #[serde(default)]
    tool: String,
    #[serde(default)]
    method: String,
    callback: Option<String>,
}

#[derive(Debug, Serialize)]
struct Answer {
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

pub async fn handler(State(state): State<Arc<AppState>>, Params(request): Params<Request>) -> Response {
    let answer = match ToolMethod::new(&request.tool, &request.method) {
        None => Answer { status: "ERROR", error: Some("No tool name supplied".to_string()) },
        Some(use_) => match log(&state, &use_, None).await {
            Ok(()) => Answer { status: "OK", error: None },
            Err(e) => {
                tracing::warn!("logger: {e:#}");
                Answer { status: "ERROR", error: Some("Could not log".to_string()) }
            }
        },
    };
    let mut response = jsonp(answer, request.callback).into_response();
    // Called from other tools' pages; there is nothing private here.
    response.headers_mut().insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
    response
}

/// Middleware for a tool here: counts a request as a use if it brings
/// parameters (not just opening the form) and does not fail.
pub async fn count_uses(state: Arc<AppState>, tool: &'static str, request: HttpRequest, next: Next) -> Response {
    let with_parameters = has_parameters(request.method(), request.uri());
    let response = next.run(request).await;
    if with_parameters && response.status().as_u16() < 400 {
        // In the background: counting must not slow down or break the tool.
        tokio::spawn(async move {
            let use_ = ToolMethod { tool: THIS_TOOL.to_string(), method: tool.to_string() };
            if let Err(e) = log(&state, &use_, Some(tool)).await {
                tracing::warn!("logger: cannot count a use of {tool}: {e:#}");
            }
        });
    }
    response
}

fn has_parameters(method: &Method, uri: &axum::http::Uri) -> bool {
    method == Method::POST || uri.query().is_some_and(|q| !q.is_empty())
}

/// Counts a use; `main_tool` names this tool's row in the `tool` table, to link the two.
async fn log(state: &AppState, use_: &ToolMethod, main_tool: Option<&str>) -> anyhow::Result<()> {
    let today: u32 = Utc::now().format("%Y%m%d").to_string().parse()?;
    let mut conn = state.tool_db.conn().await?;
    // The id of the existing row, or of a new one.
    conn.exec_drop(
        "INSERT INTO logging_tools (name, method, start_log, main_tool_id)
         VALUES (?, ?, ?, (SELECT id FROM tool WHERE subdomain=? AND name=? LIMIT 1))
         ON DUPLICATE KEY UPDATE id=LAST_INSERT_ID(id), main_tool_id=COALESCE(main_tool_id, VALUES(main_tool_id))",
        (&use_.tool, &use_.method, today, THIS_TOOL, main_tool),
    )
    .await?;
    let tool_id = conn.last_insert_id().ok_or_else(|| anyhow::anyhow!("no logging_tools id"))?;
    conn.exec_drop(
        "INSERT INTO logging_uses (tool_id, date, used) VALUES (?, ?, 1) ON DUPLICATE KEY UPDATE used=used+1",
        (tool_id, today),
    )
    .await?;
    Ok(())
}

/// A tool and method name, normalised as the log has always stored them.
#[derive(Debug, PartialEq, Eq)]
struct ToolMethod {
    tool: String,
    method: String,
}

fn regex(pattern: &str) -> Regex {
    // The patterns are constants, covered by the tests.
    #[allow(clippy::expect_used)]
    Regex::new(pattern).expect("valid regex")
}

/// Methods are cut at the first character that starts parameters or SQL.
static METHOD_END: LazyLock<Regex> = LazyLock::new(|| regex(r#"\s*[()"'=;,].*$"#));
static MIXNMATCH_SQL: LazyLock<Regex> = LazyLock::new(|| regex(r"(?i)\s+(and|union)\b.*"));
static ICOMMONS_CATEGORY: LazyLock<Regex> = LazyLock::new(|| regex(r"^category\b"));

impl ToolMethod {
    fn new(tool: &str, method: &str) -> Option<Self> {
        let tool = tool.trim().to_lowercase();
        let mut method = METHOD_END.replace(method, "").into_owned();
        match tool.as_str() {
            "mix-n-match" => {
                method = MIXNMATCH_SQL.replace(&method, "").into_owned();
                method = method.split('.').next().unwrap_or_default().to_string();
            }
            "icommons" if ICOMMONS_CATEGORY.is_match(&method) => method = "category".to_string(),
            _ => {}
        }
        let truncate = |s: &str| s.trim().to_lowercase().chars().take(MAX_NAME).collect::<String>();
        (!tool.is_empty()).then(|| Self { tool: truncate(&tool), method: truncate(&method) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn normalised(tool: &str, method: &str) -> Option<(String, String)> {
        ToolMethod::new(tool, method).map(|t| (t.tool, t.method))
    }

    #[test]
    fn uses_bring_parameters() {
        let uri = |s: &str| s.parse::<axum::http::Uri>().unwrap();
        assert!(!has_parameters(&Method::GET, &uri("/spacer/")));
        assert!(!has_parameters(&Method::GET, &uri("/spacer/?")));
        assert!(has_parameters(&Method::GET, &uri("/spacer/?id=1957-001A")));
        assert!(has_parameters(&Method::POST, &uri("/tab2wiki/")));
    }

    #[test]
    fn names_as_stored() {
        let pair = |a: &str, b: &str| Some((a.to_string(), b.to_string()));
        assert_eq!(normalised(" Flickr2Commons ", "Upload to Commons"), pair("flickr2commons", "upload to commons"));
        assert_eq!(normalised("x", "search (q=foo)"), pair("x", "search"));
        assert_eq!(normalised("x", "a;DROP TABLE"), pair("x", "a"));
        assert_eq!(normalised("mix-n-match", "catalog 5 and 1=1"), pair("mix-n-match", "catalog 5"));
        assert_eq!(normalised("mix-n-match", "foo.bar"), pair("mix-n-match", "foo"));
        assert_eq!(normalised("icommons", "category Foo"), pair("icommons", "category"));
        assert_eq!(normalised("  ", "x"), None);
        assert_eq!(normalised("t", &"m".repeat(200)).map(|(_, m)| m.len()), Some(128));
    }
}
