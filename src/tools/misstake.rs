//! `misstake.php`: the red links on some pages, ranked by how often the
//! whole wiki links to them; the missing articles most wanted.

use crate::mediawiki;
use crate::replicas::{display_title, select_in};
use crate::state::AppState;
use crate::web::{AppError, Params, render, thousands};
use crate::wiki::Wiki;
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use serde::Deserialize;
use std::sync::Arc;

const NS_CATEGORY: i64 = 14;
const MAX_LINES: usize = 50;
/// Pages to take the red links from.
const MAX_PAGES: usize = 5000;
const MAX_SHOWN: usize = 1000;
/// Red link candidates to look up.
const MAX_TARGETS: usize = 50_000;

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Request {
    language: String,
    project: String,
    page: String,
    doit: Option<String>,
}

impl Default for Request {
    fn default() -> Self {
        Self { language: "en".to_string(), project: "wikipedia".to_string(), page: String::new(), doit: None }
    }
}

struct Wanted {
    title: String,
    links_url: String,
    links: String,
}

struct Results {
    pages: usize,
    red_links: usize,
    wanted: Vec<Wanted>,
}

#[derive(Template)]
#[template(path = "tools/misstake.html")]
struct Page<'a> {
    title: &'static str,
    request: &'a Request,
    results: Option<Results>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let lines: Vec<&str> = request.page.lines().map(str::trim).filter(|l| !l.is_empty()).take(MAX_LINES).collect();
    let results = match lines.is_empty() {
        true => None,
        false => {
            let wiki = Wiki::new(&request.language, &request.project)?;
            Some(results(&state, &wiki, &lines).await?)
        }
    };
    render(&Page { title: "Ranked missing articles", request: &request, results })
}

async fn results(state: &AppState, wiki: &Wiki, lines: &[&str]) -> Result<Results, AppError> {
    let mut page_ids = vec![];
    let mut categories = vec![];
    for (id, namespace, title) in resolve(&state.http, wiki, lines).await? {
        match namespace {
            NS_CATEGORY => categories.push(title),
            _ => page_ids.push(id),
        }
    }
    let mut links = state.replicas.conn(wiki, &["categorylinks", "linktarget", "pagelinks"]).await?;
    if !categories.is_empty() {
        let sql = "SELECT DISTINCT cl_from FROM categorylinks JOIN linktarget ON cl_target_id=lt_id
            WHERE lt_namespace=14 AND lt_title IN";
        let limit = format!("LIMIT {MAX_PAGES}");
        page_ids.extend(select_in::<u64, _>(&mut links, sql, &limit, &[], &categories).await?);
    }
    page_ids.sort_unstable();
    page_ids.dedup();
    page_ids.truncate(MAX_PAGES);

    let sql = "SELECT DISTINCT lt_title FROM pagelinks JOIN linktarget ON pl_target_id=lt_id
        WHERE lt_namespace=0 AND pl_from IN";
    let limit = format!("LIMIT {MAX_TARGETS}");
    let mut targets: Vec<String> = select_in(&mut links, sql, &limit, &[], &page_ids).await?;
    targets.truncate(MAX_TARGETS);
    let existing = state.replicas.existing(wiki, 0, &targets).await?;
    let red: Vec<String> = targets.into_iter().filter(|t| !existing.contains(t)).collect();

    let mut counts: Vec<(String, u64)> = state.replicas.link_counts(wiki, &red, false).await?.into_iter().collect();
    counts.sort_by(|(a, x), (b, y)| y.cmp(x).then_with(|| a.cmp(b)));
    let wanted = counts
        .into_iter()
        .take(MAX_SHOWN)
        .map(|(title, links)| Wanted {
            links_url: wiki.page_url(&format!("Special:WhatLinksHere/{title}")),
            title: display_title(&title),
            links: thousands(links),
        })
        .collect();
    Ok(Results { pages: page_ids.len(), red_links: red.len(), wanted })
}

/// `(page id, namespace, database title)` of the pages that exist; the API
/// knows the wiki's namespace names.
async fn resolve(http: &reqwest::Client, wiki: &Wiki, titles: &[&str]) -> anyhow::Result<Vec<(u64, i64, String)>> {
    let mut pages = vec![];
    for batch in titles.chunks(MAX_LINES) {
        let titles = batch.join("|");
        let json = mediawiki::api(http, wiki, &[("action", "query"), ("titles", titles.as_str())]).await?;
        for page in json["query"]["pages"].as_array().into_iter().flatten() {
            let (Some(id), Some(namespace), Some(title)) =
                (page["pageid"].as_u64(), page["ns"].as_i64(), page["title"].as_str())
            else {
                continue;
            };
            pages.push((id, namespace, database_title(namespace, title)));
        }
    }
    Ok(pages)
}

/// An API title as the database has it: without namespace prefix, with underscores.
fn database_title(namespace: i64, title: &str) -> String {
    let title = match namespace {
        0 => title,
        _ => title.split_once(':').map_or(title, |(_, rest)| rest),
    };
    title.replace(' ', "_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_titles() {
        assert_eq!(database_title(0, "Star Trek: Voyager"), "Star_Trek:_Voyager");
        assert_eq!(database_title(14, "Kategorie:Leuchtturm in Schottland"), "Leuchtturm_in_Schottland");
    }
}
