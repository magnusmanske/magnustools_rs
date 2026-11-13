//! `artniw.php`: pages without interwikis, optionally inside a category
//! tree and among pages that use a template. Unlike the original, the
//! category tree is searched in the chosen namespace, not only in articles.

use crate::mediawiki;
use crate::replicas::select_in;
use crate::state::AppState;
use crate::tools::interwiki::{self, MAX_RESULTS};
use crate::web::{AppError, Params, render};
use crate::wiki::{Wiki, percent_encode};
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
    #[serde(default = "default_namespace")]
    ns: String,
    #[serde(default)]
    cat: String,
    #[serde(default)]
    dpt: Option<String>,
    #[serde(default)]
    tpl: String,
    #[serde(default)]
    sub: Option<String>,
    #[serde(default)]
    go: Option<String>,
}

fn default_project() -> String {
    "wiki".to_string()
}

fn default_namespace() -> String {
    "0".to_string()
}

struct Row {
    page: String,
    url: String,
    search_url: String,
}

struct Results {
    rows: Vec<Row>,
    wiki: String,
    truncated: bool,
}

#[derive(Template)]
#[template(path = "tools/artniw.html")]
struct Page<'a> {
    title: &'static str,
    request: &'a Request,
    depth: u32,
    include_subpages: bool,
    results: Option<Results>,
    error: Option<String>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let depth = interwiki::depth(request.dpt.as_deref());
    let include_subpages = request.sub.as_deref().is_some_and(|value| !value.is_empty() && value != "0");
    let mut page = Page {
        title: "Articles without interwikis [Joanjoc]",
        request: &request,
        depth,
        include_subpages,
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
    let namespace = match request.ns.trim().parse::<i64>() {
        Ok(namespace) if namespace >= 0 => namespace,
        _ => {
            page.error = Some(format!("“{}” is not a namespace ID.", request.ns.trim()));
            return render(&page);
        }
    };
    page.results = Some(results(&state, &request, &language, namespace, depth, include_subpages).await?);
    render(&page)
}

async fn results(
    state: &AppState,
    request: &Request,
    language: &str,
    namespace: i64,
    depth: u32,
    include_subpages: bool,
) -> Result<Results, AppError> {
    let project = interwiki::project(&request.pr);
    let wiki = Wiki::new(language, project)?;
    let api = state.api(&wiki).await?;
    let template = match request.tpl.trim() {
        "" => None,
        input => Some(interwiki::db_title_without_namespace(input, &api)),
    };
    let category = match request.cat.trim() {
        "" => String::new(),
        input => interwiki::db_title_without_namespace(input, &api),
    };
    let candidates = interwiki::candidates(state, &wiki, &category, depth, namespace).await?;
    let truncated = candidates.as_ref().is_some_and(|pages| pages.len() >= interwiki::MAX_CANDIDATES);

    let mut conn = state.replicas.conn(&wiki, &["page", "langlinks", "templatelinks", "linktarget"]).await?;
    let subpages = match include_subpages {
        true => "",
        false => "AND p.page_title NOT LIKE '%/%'",
    };
    let condition = interwiki::template_condition(template.as_deref());
    let base = format!(
        "SELECT DISTINCT p.page_title FROM page p
         WHERE p.page_namespace=? AND p.page_is_redirect=0 {subpages}
         {condition}
         AND NOT EXISTS (SELECT 1 FROM langlinks ll WHERE ll.ll_from=p.page_id)"
    );
    let mut params = vec![Value::from(namespace)];
    if let Some(template) = &template {
        params.push(Value::from(template.as_str()));
    }
    let mut titles: Vec<String> = match &candidates {
        None => {
            let sql = format!("{base} ORDER BY p.page_title LIMIT {MAX_RESULTS}");
            conn.exec(sql, wikimisc::mysql_async::Params::Positional(params)).await.map_err(anyhow::Error::from)?
        }
        Some(candidates) => {
            let sql = format!("{base} AND p.page_title IN");
            let suffix = format!("ORDER BY p.page_title LIMIT {MAX_RESULTS}");
            select_in(&mut conn, &sql, &suffix, &params, candidates).await?
        }
    };
    titles.sort_unstable();
    titles.dedup();
    titles.truncate(MAX_RESULTS);

    let rows = titles
        .into_iter()
        .map(|title| {
            let title = mediawiki::Title::new(&title, namespace);
            let page = title.full_pretty(&api).unwrap_or_else(|| title.pretty().to_string());
            let search = format!("{page} site:*.{project}.org -site:{language}.{project}.org");
            Row {
                url: crate::wiki::page_url(wiki.host(), &page),
                search_url: format!("https://www.google.com/search?q={}", percent_encode(&search)),
                page,
            }
        })
        .collect();
    Ok(Results { rows, wiki: wiki.to_string(), truncated })
}
