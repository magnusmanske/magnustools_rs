//! `sugart.php`: articles with many interwikis that are missing a link to
//! one language, optionally inside a category tree and among pages that use
//! a template. Redirects are left out, as they are not articles.

use crate::mediawiki;
use crate::replicas::select_in;
use crate::state::AppState;
use crate::tools::interwiki::{self, MAX_RESULTS};
use crate::web::{AppError, Params, render};
use crate::wiki::Wiki;
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use serde::Deserialize;
use std::sync::Arc;
use wikimisc::mysql_async::Value;
use wikimisc::mysql_async::prelude::Queryable;

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default)]
    l1: String,
    #[serde(default = "default_project")]
    pr: String,
    #[serde(default)]
    l2: String,
    #[serde(default)]
    cat: String,
    #[serde(default)]
    dpt: Option<String>,
    #[serde(default)]
    tpl: String,
    #[serde(default)]
    go: Option<String>,
}

fn default_project() -> String {
    "wiki".to_string()
}

struct Row {
    count: u64,
    page: String,
    url: String,
    search_url: String,
}

struct Results {
    rows: Vec<Row>,
    wiki: String,
    missing_language: String,
    truncated: bool,
}

#[derive(Template)]
#[template(path = "tools/sugart.html")]
struct Page<'a> {
    title: &'static str,
    request: &'a Request,
    depth: u32,
    results: Option<Results>,
    error: Option<String>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let depth = interwiki::depth(request.dpt.as_deref());
    let mut page = Page {
        title: "Suggest articles from interwikis [Joanjoc]",
        request: &request,
        depth,
        results: None,
        error: None,
    };
    if request.go.is_none() && request.l1.trim().is_empty() {
        return render(&page);
    }
    let language = request.l1.trim().to_lowercase();
    if language.is_empty() {
        page.error = Some("Please give the reference language.".to_string());
        return render(&page);
    }
    let missing = request.l2.trim().to_lowercase();
    if missing.is_empty() {
        page.error = Some("Please give the language the articles should link to.".to_string());
        return render(&page);
    }
    page.results = Some(results(&state, &request, &language, &missing, depth).await?);
    render(&page)
}

async fn results(
    state: &AppState,
    request: &Request,
    language: &str,
    missing: &str,
    depth: u32,
) -> Result<Results, AppError> {
    let project = interwiki::project(&request.pr);
    let wiki = Wiki::new(language, project)?;
    let missing_wiki = Wiki::new(missing, project)?;
    let api = state.api(&wiki).await?;
    let template = match request.tpl.trim() {
        "" => None,
        input => Some(interwiki::db_title_without_namespace(input, &api)),
    };
    let category = match request.cat.trim() {
        "" => String::new(),
        input => interwiki::db_title_without_namespace(input, &api),
    };
    let candidates = interwiki::candidates(state, &wiki, &category, depth, 0).await?;
    let truncated = candidates.as_ref().is_some_and(|pages| pages.len() >= interwiki::MAX_CANDIDATES);

    let mut conn = state.replicas.conn(&wiki, &["page", "langlinks", "templatelinks", "linktarget"]).await?;
    let condition = interwiki::template_condition(template.as_deref());
    let base = format!(
        "SELECT COUNT(*) AS count, p.page_title FROM page p
         JOIN langlinks l1 ON l1.ll_from=p.page_id
         WHERE p.page_namespace=0 AND p.page_is_redirect=0 {condition}
         AND NOT EXISTS (SELECT 1 FROM langlinks l2 WHERE l2.ll_from=l1.ll_from AND l2.ll_lang=?)"
    );
    let group = "GROUP BY p.page_id, p.page_title ORDER BY count(*) DESC, p.page_title";
    let mut params = vec![];
    if let Some(template) = &template {
        params.push(Value::from(template.as_str()));
    }
    params.push(Value::from(missing));
    let mut rows: Vec<(u64, String)> = match &candidates {
        None => {
            let sql = format!("{base} {group} LIMIT {MAX_RESULTS}");
            conn.exec(sql, wikimisc::mysql_async::Params::Positional(params)).await.map_err(anyhow::Error::from)?
        }
        Some(candidates) => {
            let sql = format!("{base} AND p.page_title IN");
            let suffix = format!("{group} LIMIT {MAX_RESULTS}");
            select_in(&mut conn, &sql, &suffix, &params, candidates).await?
        }
    };
    rows.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    rows.truncate(MAX_RESULTS);

    let rows = rows
        .into_iter()
        .map(|(count, title)| {
            let page = mediawiki::Title::new(&title, 0).pretty().to_string();
            Row {
                count,
                url: crate::wiki::page_url(wiki.host(), &page),
                search_url: crate::wiki::page_url(missing_wiki.host(), &format!("Special:Search/{page}")),
                page,
            }
        })
        .collect();
    Ok(Results { rows, wiki: wiki.to_string(), missing_language: missing.to_string(), truncated })
}
