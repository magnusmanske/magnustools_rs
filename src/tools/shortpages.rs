//! `shortpages.php`: the pages shorter than 50 bytes, optionally without
//! templates or with a single author.

use crate::mediawiki;
use crate::state::AppState;
use crate::web::{AppError, Pager, Params, render};
use crate::wiki::{Wiki, encode_component};
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use serde::Deserialize;
use std::sync::Arc;
use wikimisc::mysql_async::prelude::Queryable;

const MAX_LENGTH: u32 = 50;
const DEFAULT_LIMIT: u64 = 100;
const MAX_LIMIT: u64 = 500;

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default = "default_language", alias = "language")]
    lang: String,
    #[serde(default = "default_project", alias = "project")]
    family: String,
    #[serde(default, alias = "namespace")]
    namespaces: String,
    #[serde(default)]
    invert: Option<String>,
    #[serde(default)]
    filter: String,
    limit: Option<String>,
    offset: Option<String>,
    doit: Option<String>,
}

fn default_language() -> String {
    "en".to_string()
}

fn default_project() -> String {
    "wikipedia".to_string()
}

struct Row {
    page: String,
    url: String,
    length: u32,
}

struct Results {
    rows: Vec<Row>,
    wiki: String,
    pager: Pager,
}

#[derive(Template)]
#[template(path = "tools/shortpages.html")]
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
    let mut page =
        Page { title: "Short pages [Erwin85]", request: &request, limit, invert, results: None, error: None };
    let namespace = match request.namespaces.trim() {
        "" => 0,
        value => match value.parse() {
            Ok(namespace) => namespace,
            Err(_) => {
                page.error = Some(format!("“{value}” is not a namespace ID."));
                return render(&page);
            }
        },
    };
    let submitted = request.doit.is_some()
        || !request.namespaces.trim().is_empty()
        || !request.filter.trim().is_empty()
        || request.limit.is_some()
        || request.offset.is_some()
        || request.invert.is_some();
    page.results = match submitted {
        true => Some(results(&state, &request, namespace, invert, limit, offset).await?),
        false => None,
    };
    render(&page)
}

async fn results(
    state: &AppState,
    request: &Request,
    namespace: i64,
    invert: bool,
    limit: u64,
    offset: u64,
) -> Result<Results, AppError> {
    let wiki = Wiki::new(&request.lang, &request.family)?;
    let api = state.api(&wiki).await?;
    let mut conn = state.replicas.conn(&wiki, &["page", "templatelinks", "revision"]).await?;
    let namespace_condition = match (namespace, invert) {
        (-1, _) => "",
        (_, true) => "AND page_namespace != ?",
        (_, false) => "AND page_namespace = ?",
    };
    let filter = match request.filter.trim() {
        "notemplates" => "AND NOT EXISTS (SELECT 1 FROM templatelinks WHERE tl_from=page_id)",
        "su" => "AND (SELECT COUNT(DISTINCT rev_actor) FROM revision WHERE rev_page=page_id) = 1",
        _ => "",
    };
    let sql = format!(
        "SELECT page_namespace, page_title, page_len FROM page
         WHERE page_len < {MAX_LENGTH} AND page_is_redirect=0 {namespace_condition} {filter}
         ORDER BY page_len ASC, page_title ASC LIMIT {limit} OFFSET {offset}"
    );
    let rows: Vec<(i64, String, u32)> = match namespace {
        -1 => conn.exec(sql, ()).await,
        ns => conn.exec(sql, (ns,)).await,
    }
    .map_err(anyhow::Error::from)?;
    let more = rows.len() as u64 == limit;
    let rows = rows
        .into_iter()
        .map(|(namespace, title, length)| {
            let title = mediawiki::Title::new(&title, namespace);
            let page = title.full_pretty(&api).unwrap_or_else(|| title.pretty().to_string());
            Row { url: crate::wiki::page_url(wiki.host(), &page), page, length }
        })
        .collect();
    let pager = Pager::previous_next(
        (offset > 0).then(|| page_url(request, namespace, invert, limit, offset.saturating_sub(limit))),
        more.then(|| page_url(request, namespace, invert, limit, offset.saturating_add(limit))),
        format!("Previous {limit}"),
        format!("Next {limit}"),
    );
    Ok(Results { rows, wiki: wiki.to_string(), pager })
}

fn page_url(request: &Request, namespace: i64, invert: bool, limit: u64, offset: u64) -> String {
    let namespace = namespace.to_string();
    let limit = limit.to_string();
    let offset = offset.to_string();
    let mut params = vec![
        ("language", request.lang.as_str()),
        ("project", request.family.as_str()),
        ("namespaces", namespace.as_str()),
        ("filter", request.filter.as_str()),
        ("limit", limit.as_str()),
        ("offset", offset.as_str()),
        ("doit", "1"),
    ];
    if invert {
        params.push(("invert", "1"));
    }
    let query: Vec<String> = params.iter().map(|(k, v)| format!("{k}={}", encode_component(v))).collect();
    format!("/shortpages/?{}", query.join("&"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pagination_keeps_the_query() {
        let request = Request {
            lang: "de".into(),
            family: "wikipedia".into(),
            namespaces: "0".into(),
            invert: Some("1".into()),
            filter: "su".into(),
            limit: None,
            offset: None,
            doit: Some("1".into()),
        };
        assert_eq!(
            page_url(&request, 0, true, 50, 100),
            "/shortpages/?language=de&project=wikipedia&namespaces=0&filter=su&limit=50&offset=100&doit=1&invert=1"
        );
    }
}
