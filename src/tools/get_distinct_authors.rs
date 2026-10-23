//! `get_distinct_authors.php`: the registered users who edited a set of
//! articles, with their numbers of edits, as plain text.

use crate::replicas::{db_title, select_in};
use crate::state::AppState;
use crate::web::{AppError, Params, render};
use crate::wiki::Wiki;
use askama::Template;
use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;

const MAX_PAGES: usize = 5000;

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default = "default_language")]
    language: String,
    #[serde(default = "default_project")]
    project: String,
    #[serde(default)]
    page: String,
    #[serde(default)]
    pages: String,
}

fn default_language() -> String {
    "en".to_string()
}

fn default_project() -> String {
    "wikipedia".to_string()
}

#[derive(Template)]
#[template(path = "tools/get_distinct_authors.html")]
struct Page<'a> {
    title: &'static str,
    request: &'a Request,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Response, AppError> {
    let titles: Vec<String> = request
        .pages
        .split(['|', '\n'])
        .chain([request.page.as_str()])
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(db_title)
        .take(MAX_PAGES)
        .collect();
    if titles.is_empty() {
        return Ok(render(&Page { title: "Distinct authors", request: &request })?.into_response());
    }
    let wiki = Wiki::new(&request.language, &request.project)?;
    let mut conn = state.replicas.conn(&wiki, &["page", "revision", "actor"]).await?;
    let sql = "SELECT page_id FROM page WHERE page_namespace=0 AND page_title IN";
    let page_ids: Vec<u64> = select_in(&mut conn, sql, "", &[], &titles).await?;
    let sql = "SELECT actor_name, COUNT(*) FROM revision_userindex JOIN actor ON actor_id=rev_actor
        WHERE actor_user IS NOT NULL AND rev_page IN";
    let rows: Vec<(String, u64)> = select_in(&mut conn, sql, "GROUP BY actor_name", &[], &page_ids).await?;
    // Chunks of pages may count the same author more than once.
    let mut edits: HashMap<String, u64> = HashMap::new();
    for (author, count) in rows {
        *edits.entry(author).or_default() += count;
    }
    Ok(([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], authors_list(edits)).into_response())
}

/// Most edits first; equal counts by name.
fn authors_list(edits: HashMap<String, u64>) -> String {
    let mut edits: Vec<(String, u64)> = edits.into_iter().collect();
    edits.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    edits.iter().map(|(author, count)| format!("{author}\t({count} edits)\n")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list() {
        let edits = HashMap::from([("B".to_string(), 2), ("A".to_string(), 2), ("C".to_string(), 5)]);
        assert_eq!(authors_list(edits), "C\t(5 edits)\nA\t(2 edits)\nB\t(2 edits)\n");
    }
}
