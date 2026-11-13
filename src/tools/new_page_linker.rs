//! `new-page-linker`: new articles, the pages that already link to them,
//! and search results that might want a link.

use crate::state::AppState;
use crate::web::{AppError, Params, render};
use crate::wiki::Wiki;
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use serde::Deserialize;
use std::sync::Arc;
use tokio::task::JoinSet;

const DEFAULT_NUMBER: usize = 10;
const MAX_NUMBER: usize = 100;
const MAX_LINKS: usize = 500;

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default)]
    language: String,
    number: Option<String>,
}

impl Request {
    fn number(&self) -> usize {
        self.number.as_deref().and_then(|n| n.trim().parse().ok()).unwrap_or(DEFAULT_NUMBER).clamp(1, MAX_NUMBER)
    }
}

struct LinkPage {
    title: String,
    url: String,
}

struct Row {
    title: String,
    url: String,
    backlinks: Vec<LinkPage>,
    search: Vec<LinkPage>,
}

#[derive(Template)]
#[template(path = "tools/new_page_linker.html")]
struct Page<'a> {
    title: &'static str,
    request: &'a Request,
    number: usize,
    wiki: String,
    rows: Vec<Row>,
    error: Option<String>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let language = request.language.trim().to_lowercase();
    let number = request.number();
    let mut page =
        Page { title: "New page linker", request: &request, number, wiki: language.clone(), rows: vec![], error: None };
    if language.is_empty() {
        return render(&page);
    }
    let wiki = match Wiki::new(&language, "wikipedia") {
        Ok(wiki) => wiki,
        Err(e) => {
            page.error = Some(format!("Not a wiki: {e}."));
            return render(&page);
        }
    };
    let titles = new_pages(&state, &wiki, number).await?;
    let mut tasks = JoinSet::new();
    for (index, title) in titles.into_iter().enumerate() {
        let state = state.clone();
        let wiki = wiki.clone();
        tasks.spawn(async move {
            let backlinks = backlinks(&state, &wiki, &title).await.unwrap_or_default();
            let search = search(&state, &wiki, &title).await.unwrap_or_default();
            (index, title, backlinks, search)
        });
    }
    let mut rows: Vec<(usize, Row)> = vec![];
    while let Some(result) = tasks.join_next().await {
        let Ok((index, title, backlinks, search)) = result else { continue };
        let without_backlinks =
            search.into_iter().filter(|hit| !backlinks.iter().any(|b| b.title == hit.title)).collect();
        rows.push((index, Row { url: wiki.page_url(&title), title, backlinks, search: without_backlinks }));
    }
    rows.sort_by_key(|(index, _)| *index);
    page.rows = rows.into_iter().map(|(_, row)| row).collect();
    render(&page)
}

async fn new_pages(state: &AppState, wiki: &Wiki, number: usize) -> Result<Vec<String>, AppError> {
    let limit = number.to_string();
    let json = crate::mediawiki::api(
        &state.http,
        wiki,
        &[
            ("action", "query"),
            ("list", "recentchanges"),
            ("rcnamespace", "0"),
            ("rclimit", limit.as_str()),
            ("rcprop", "title|redirect"),
            ("rctype", "new"),
        ],
    )
    .await?;
    Ok(json["query"]["recentchanges"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|change| !change["redirect"].as_bool().unwrap_or(false) && !change["redirect"].is_object())
        .filter_map(|change| change["title"].as_str().map(str::to_string))
        .collect())
}

async fn backlinks(state: &AppState, wiki: &Wiki, title: &str) -> Result<Vec<LinkPage>, AppError> {
    let limit = MAX_LINKS.to_string();
    let json = crate::mediawiki::api(
        &state.http,
        wiki,
        &[
            ("action", "query"),
            ("generator", "backlinks"),
            ("gbltitle", title),
            ("gblnamespace", "0"),
            ("gbllimit", limit.as_str()),
            ("prop", "info"),
        ],
    )
    .await?;
    Ok(link_pages(&json, wiki, title))
}

async fn search(state: &AppState, wiki: &Wiki, title: &str) -> Result<Vec<LinkPage>, AppError> {
    let query = title.split(" (").next().unwrap_or(title).trim().replace('"', " ");
    if query.trim().is_empty() {
        return Ok(vec![]);
    }
    let search = format!("\"{}\"", query.trim());
    let limit = MAX_LINKS.to_string();
    let json = crate::mediawiki::api(
        &state.http,
        wiki,
        &[
            ("action", "query"),
            ("generator", "search"),
            ("gsrsearch", search.as_str()),
            ("gsrnamespace", "0"),
            ("gsrlimit", limit.as_str()),
            ("prop", "info"),
        ],
    )
    .await?;
    Ok(link_pages(&json, wiki, title))
}

fn link_pages(json: &serde_json::Value, wiki: &Wiki, exclude: &str) -> Vec<LinkPage> {
    let mut pages: Vec<LinkPage> = json["query"]["pages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|page| {
            let title = page["title"].as_str()?;
            if title == exclude {
                return None;
            }
            let mut url = wiki.page_url(title);
            if page["redirect"].as_bool().unwrap_or(false) || page["redirect"].is_object() {
                url.push_str("?redirect=no");
            }
            Some(LinkPage { title: title.to_string(), url })
        })
        .collect();
    pages.sort_by(|a, b| a.title.cmp(&b.title));
    pages.truncate(MAX_LINKS);
    pages
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_clamped() {
        let request = |number: Option<&str>| Request { language: "en".into(), number: number.map(str::to_string) };
        assert_eq!(request(None).number(), DEFAULT_NUMBER);
        assert_eq!(request(Some("0")).number(), 1);
        assert_eq!(request(Some("1000")).number(), MAX_NUMBER);
        assert_eq!(request(Some("bad")).number(), DEFAULT_NUMBER);
    }

    #[test]
    fn search_queries_drop_disambiguation_suffixes() {
        let title = "Mercury (planet)";
        assert_eq!(title.split(" (").next().unwrap(), "Mercury");
    }
}
