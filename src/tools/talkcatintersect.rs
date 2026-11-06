//! `talkcatintersect.php`: articles in one category tree whose talk pages
//! are in another category.

use crate::mediawiki;
use crate::replicas::{display_title, select_in};
use crate::state::AppState;
use crate::web::{AppError, Params, render};
use crate::wiki::Wiki;
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use serde::Deserialize;
use std::sync::Arc;
use wikimisc::mysql_async::Value;

const DEFAULT_DEPTH: u32 = 10;
const MAX_DEPTH: u32 = 10;
const DEFAULT_LIMIT: u64 = 100;
const MAX_LIMIT: u64 = 500;
/// Articles to look at; a category tree can hold far more than a page can show.
const MAX_ARTICLES: usize = 20_000;

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default = "default_language", alias = "language")]
    lang: String,
    #[serde(default = "default_project", alias = "project")]
    family: String,
    #[serde(default)]
    pcategory: String,
    #[serde(default)]
    tpcategory: String,
    #[serde(default)]
    d: Option<String>,
    limit: Option<String>,
}

fn default_language() -> String {
    "en".to_string()
}

fn default_project() -> String {
    "wikipedia".to_string()
}

struct Entry {
    page: String,
    page_url: String,
    talk_url: String,
}

struct Results {
    entries: Vec<Entry>,
    wiki: String,
    articles: usize,
    truncated: bool,
}

#[derive(Template)]
#[template(path = "tools/talkcatintersect.html")]
struct Page<'a> {
    title: &'static str,
    request: &'a Request,
    depth: u32,
    limit: u64,
    results: Option<Results>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let depth = request.d.as_deref().and_then(|d| d.trim().parse().ok()).unwrap_or(DEFAULT_DEPTH).clamp(1, MAX_DEPTH);
    let limit =
        request.limit.as_deref().and_then(|l| l.trim().parse().ok()).unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let results = match (request.pcategory.trim().is_empty(), request.tpcategory.trim().is_empty()) {
        (false, false) => Some(results(&state, &request, depth, limit).await?),
        _ => None,
    };
    render(&Page { title: "Talk page category intersect [Erwin85]", request: &request, depth, limit, results })
}

async fn results(state: &AppState, request: &Request, depth: u32, limit: u64) -> Result<Results, AppError> {
    let wiki = Wiki::new(&request.lang, &request.family)?;
    let api = state.api(&wiki).await?;
    let articles =
        state.replicas.some_pages_in_tree(&wiki, request.pcategory.trim(), depth, 0, Some(MAX_ARTICLES)).await?;
    let mut conn = state.replicas.conn(&wiki, &["page", "categorylinks", "linktarget"]).await?;
    let sql = "SELECT DISTINCT p.page_title FROM page p
        JOIN page tp ON tp.page_title=p.page_title AND tp.page_namespace=1
        JOIN categorylinks cl ON cl.cl_from=tp.page_id AND cl.cl_type='page'
        JOIN linktarget lt ON lt.lt_id=cl.cl_target_id
        WHERE p.page_namespace=0 AND lt.lt_namespace=14 AND lt.lt_title=? AND p.page_title IN";
    let suffix = format!("ORDER BY p.page_title LIMIT {limit}");
    let params = [Value::from(category_title(&request.tpcategory, &api))];
    let titles: Vec<String> = select_in(&mut conn, sql, &suffix, &params, &articles).await?;
    let entries = titles
        .into_iter()
        .map(|title| {
            let page = display_title(&title);
            let talk = mediawiki::Title::new(&page, 1);
            let talk = talk.full_pretty(&api).unwrap_or_else(|| format!("Talk:{page}"));
            Entry {
                page_url: crate::wiki::page_url(wiki.host(), &page),
                talk_url: crate::wiki::page_url(wiki.host(), &talk),
                page,
            }
        })
        .collect();
    Ok(Results { entries, wiki: wiki.to_string(), articles: articles.len(), truncated: articles.len() >= MAX_ARTICLES })
}

/// A category name as the database has it, without a namespace prefix.
fn category_title(input: &str, api: &mediawiki::Api) -> String {
    mediawiki::Title::new_from_full(input.trim(), api).with_underscores()
}
