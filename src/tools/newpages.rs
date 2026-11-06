//! `newpages.php`: pages created within a time range on a wiki.

use crate::mediawiki;
use crate::replicas::{display_title, format_timestamp};
use crate::state::AppState;
use crate::web::{AppError, Params, render};
use crate::wiki::{Wiki, encode_component};
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use serde::Deserialize;
use std::sync::Arc;
use wikimisc::mysql_async::Value;
use wikimisc::mysql_async::prelude::Queryable;

const DEFAULT_LIMIT: u64 = 100;
const MAX_LIMIT: u64 = 500;

/// Timestamp, user, comment, title, namespace, bytes.
type PageRow = (String, String, Option<String>, String, i64, Option<u32>);

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default = "default_language", alias = "language")]
    lang: String,
    #[serde(default = "default_project", alias = "project")]
    family: String,
    #[serde(default)]
    t1: String,
    #[serde(default)]
    t2: String,
    #[serde(default, alias = "namespace")]
    namespaces: String,
    #[serde(default)]
    invert: Option<String>,
    limit: Option<String>,
    offset: Option<String>,
}

fn default_language() -> String {
    "en".to_string()
}

fn default_project() -> String {
    "wikipedia".to_string()
}

struct Entry {
    time: String,
    page: String,
    page_url: String,
    user: String,
    user_url: String,
    contribs_url: String,
    comment: String,
    bytes: Option<u32>,
}

struct Results {
    entries: Vec<Entry>,
    wiki: String,
    previous_url: Option<String>,
    next_url: Option<String>,
}

#[derive(Template)]
#[template(path = "tools/newpages.html")]
struct Page<'a> {
    title: &'static str,
    request: &'a Request,
    limit: u64,
    invert: bool,
    results: Option<Results>,
    error: Option<String>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let limit =
        request.limit.as_deref().and_then(|l| l.trim().parse().ok()).unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let offset = request.offset.as_deref().and_then(|o| o.trim().parse().ok()).unwrap_or(0);
    let invert = request.invert.as_deref().is_some_and(|v| !v.is_empty() && v != "0");
    let mut page = Page { title: "New pages [Erwin85]", request: &request, limit, invert, results: None, error: None };
    let (t1, t2) = match (timestamp(&request.t1, "From"), timestamp(&request.t2, "until")) {
        (Ok(Some(t1)), Ok(Some(t2))) => (t1, t2),
        (Ok(Some(_)), Ok(None)) | (Ok(None), Ok(Some(_))) => {
            page.error = Some("Please give both the start and the end of the period.".to_string());
            return render(&page);
        }
        (Ok(_), Ok(_)) => return render(&page),
        (Err(e), _) | (_, Err(e)) => {
            page.error = Some(e);
            return render(&page);
        }
    };
    let namespace = match request.namespaces.trim() {
        "" => 0,
        value => value.parse().map_err(|_| AppError::BadRequest(format!("“{value}” is not a namespace ID.")))?,
    };
    page.results = Some(results(&state, &request, &t1, &t2, namespace, limit, offset).await?);
    render(&page)
}

/// A full `YYYYMMDDHHMMSS` timestamp; `None` if the field is empty.
fn timestamp(value: &str, label: &str) -> Result<Option<String>, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if value.len() != 14 || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("{label} is not a timestamp in the form YYYYMMDDHHMMSS."));
    }
    Ok(Some(value.to_string()))
}

async fn results(
    state: &AppState,
    request: &Request,
    t1: &str,
    t2: &str,
    namespace: i64,
    limit: u64,
    offset: u64,
) -> Result<Results, AppError> {
    let wiki = Wiki::new(&request.lang, &request.family)?;
    let mut conn = state.replicas.conn(&wiki, &["revision", "page", "actor", "comment"]).await?;
    let invert = request.invert.as_deref().is_some_and(|v| !v.is_empty() && v != "0");
    let namespace_condition = match (namespace, invert) {
        (-1, _) => "",
        (_, true) => "AND p.page_namespace != ?",
        (_, false) => "AND p.page_namespace = ?",
    };
    let sql = format!(
        "SELECT r.rev_timestamp, a.actor_name, c.comment_text, p.page_title, p.page_namespace, r.rev_len
         FROM revision_userindex r JOIN page p ON p.page_id=r.rev_page
         JOIN actor a ON a.actor_id=r.rev_actor
         LEFT JOIN comment c ON c.comment_id=r.rev_comment_id
         WHERE r.rev_timestamp >= ? AND r.rev_timestamp <= ? AND r.rev_parent_id=0 {namespace_condition}
         ORDER BY r.rev_timestamp DESC LIMIT {limit} OFFSET {offset}"
    );
    let mut params = vec![Value::from(t1), Value::from(t2)];
    if namespace != -1 {
        params.push(namespace.into());
    }
    let rows: Vec<PageRow> =
        conn.exec(sql, wikimisc::mysql_async::Params::Positional(params)).await.map_err(anyhow::Error::from)?;
    let names = mediawiki::namespace_names(&state.http, &wiki).await.unwrap_or_default();
    let entries: Vec<Entry> = rows
        .into_iter()
        .map(|(timestamp, user, comment, title, namespace, bytes)| {
            let page = mediawiki::title_with_namespace(namespace, &display_title(&title), &names);
            Entry {
                time: format_timestamp(&timestamp),
                page_url: wiki.page_url(&page),
                page,
                user_url: wiki.page_url(&format!("User:{user}")),
                contribs_url: wiki.page_url(&format!("Special:Contributions/{user}")),
                user,
                comment: comment.unwrap_or_default(),
                bytes,
            }
        })
        .collect();
    let previous_url = (offset > 0).then(|| page_url(request, namespace, t1, t2, limit, offset.saturating_sub(limit)));
    let next_url = (entries.len() as u64 == limit)
        .then(|| page_url(request, namespace, t1, t2, limit, offset.saturating_add(limit)));
    Ok(Results { entries, wiki: wiki.to_string(), previous_url, next_url })
}

fn page_url(request: &Request, namespace: i64, t1: &str, t2: &str, limit: u64, offset: u64) -> String {
    let namespace = namespace.to_string();
    let limit = limit.to_string();
    let offset = offset.to_string();
    let mut params = vec![
        ("language", request.lang.as_str()),
        ("project", request.family.as_str()),
        ("t1", t1),
        ("t2", t2),
        ("namespaces", namespace.as_str()),
        ("limit", limit.as_str()),
        ("offset", offset.as_str()),
    ];
    if request.invert.as_deref().is_some_and(|v| !v.is_empty() && v != "0") {
        params.push(("invert", "1"));
    }
    let query: Vec<String> = params.iter().map(|(k, v)| format!("{k}={}", encode_component(v))).collect();
    format!("/newpages/?{}", query.join("&"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps() {
        assert_eq!(timestamp(" 20240102030405 ", "From").unwrap().as_deref(), Some("20240102030405"));
        assert_eq!(timestamp("", "From").unwrap(), None);
        assert!(timestamp("2024", "From").is_err());
        assert!(timestamp("2024010203040x", "From").is_err());
    }
}
