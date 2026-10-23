//! `forest4trees.php`: articles that belong in a category tree. Walks the
//! tree in one Wikipedia and its counterparts in the others; an article in
//! another language's tree whose counterpart here is not in this tree is one
//! that may be missing a category.

use crate::mediawiki;
use crate::replicas::{display_title, select_in};
use crate::state::AppState;
use crate::web::{AppError, Params, render};
use crate::wiki::Wiki;
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use serde::Deserialize;
use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use wikimisc::mysql_async::Value;

const DEFAULT_DEPTH: u32 = 3;
const MAX_DEPTH: u32 = 9;
/// Pages per tree.
const MAX_PAGES: usize = 20_000;

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Request {
    language: String,
    category: String,
    depth: Option<String>,
}

impl Default for Request {
    fn default() -> Self {
        Self { language: "en".to_string(), category: String::new(), depth: None }
    }
}

/// An article here that other languages have in their trees.
struct Missing {
    title: String,
    url: String,
    /// The articles in the other trees.
    others: Vec<(String, String, String)>,
}

struct Results {
    pages: usize,
    languages: usize,
    missing: Vec<Missing>,
}

#[derive(Template)]
#[template(path = "tools/forest4trees.html")]
struct Page<'a> {
    title: &'static str,
    request: &'a Request,
    depth: u32,
    results: Option<Results>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let depth = request.depth.as_deref().and_then(|d| d.trim().parse().ok()).unwrap_or(DEFAULT_DEPTH).min(MAX_DEPTH);
    let category = request.category.trim();
    let results = match category.is_empty() {
        true => None,
        false => {
            let wiki = Wiki::new(&request.language, "wikipedia")?;
            Some(results(&state, &wiki, category, depth).await?)
        }
    };
    render(&Page { title: "Forest for the trees", request: &request, depth, results })
}

async fn results(state: &AppState, wiki: &Wiki, category: &str, depth: u32) -> Result<Results, AppError> {
    let category = mediawiki::strip_namespace(category, &["category"]).unwrap_or(category);
    let here_tree: HashSet<String> = tree(state, wiki, category, depth).await?.into_iter().collect();
    // The language code, as `ll_lang` has it.
    let language = wiki.host().split('.').next().unwrap_or_default();

    // Article here (database title) → (language, article there, its URL).
    let mut found: BTreeMap<String, Vec<(String, String, String)>> = BTreeMap::new();
    let counterparts = category_langlinks(&state.http, wiki, category).await?;
    let mut skipped = 0;
    for (other, other_category) in &counterparts {
        let links = match links_here(state, other, other_category, depth, language).await {
            Ok(links) => links,
            Err(e) => {
                // One wiki out of reach should not spoil the rest.
                tracing::warn!("forest4trees: skipping {other}: {e:#}");
                skipped += 1;
                continue;
            }
        };
        let other_language = other.host().split('.').next().unwrap_or_default();
        for (there, here) in links {
            let here = here.replace(' ', "_");
            if !here_tree.contains(&here) {
                let url = other.page_url(&there);
                found.entry(here).or_default().push((other_language.to_string(), display_title(&there), url));
            }
        }
    }
    let mut missing: Vec<Missing> = found
        .into_iter()
        .map(|(title, others)| Missing { url: wiki.page_url(&title), title: display_title(&title), others })
        .collect();
    missing.sort_by(|a, b| b.others.len().cmp(&a.others.len()).then_with(|| a.title.cmp(&b.title)));
    Ok(Results { pages: here_tree.len(), languages: counterparts.len() - skipped, missing })
}

/// `(article there, its counterpart here)` for the articles of a tree on `other`.
async fn links_here(
    state: &AppState,
    other: &Wiki,
    category: &str,
    depth: u32,
    language: &str,
) -> anyhow::Result<Vec<(String, String)>> {
    let pages = tree(state, other, category, depth).await?;
    let mut conn = state.replicas.conn(other, &["page", "langlinks"]).await?;
    let sql = "SELECT page_title, ll_title FROM page JOIN langlinks ON ll_from=page_id
        WHERE page_namespace=0 AND ll_lang=? AND page_title IN";
    select_in(&mut conn, sql, "", &[Value::from(language)], &pages).await
}

async fn tree(state: &AppState, wiki: &Wiki, category: &str, depth: u32) -> anyhow::Result<Vec<String>> {
    let mut pages = state.replicas.pages_in_tree(wiki, category, depth, 0).await?;
    pages.truncate(MAX_PAGES);
    Ok(pages)
}

/// The category in other Wikipedias: wiki, category name. The wiki comes
/// from the link's URL, as language codes and wiki names differ (`nb`, `no`).
async fn category_langlinks(
    http: &reqwest::Client,
    wiki: &Wiki,
    category: &str,
) -> anyhow::Result<Vec<(Wiki, String)>> {
    let title = format!("Category:{category}");
    let params =
        [("action", "query"), ("titles", title.as_str()), ("prop", "langlinks"), ("llprop", "url"), ("lllimit", "max")];
    let json = mediawiki::api(http, wiki, &params).await?;
    Ok(json["query"]["pages"][0]["langlinks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|ll| {
            let other = wiki_of_url(ll["url"].as_str()?)?;
            // The title has the other wiki's namespace name.
            Some((other, ll["title"].as_str()?.split_once(':')?.1.to_string()))
        })
        .collect())
}

/// The wiki of a page URL such as `https://no.wikipedia.org/wiki/…`.
fn wiki_of_url(url: &str) -> Option<Wiki> {
    let host = url.strip_prefix("https://")?.split('/').next()?;
    Wiki::from_domain_prefix(host.strip_suffix(".org")?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wikis_from_urls() {
        let wiki = wiki_of_url("https://no.wikipedia.org/wiki/Kategori:Fyr_i_Skottland").unwrap();
        assert_eq!(wiki.dbname(), "nowiki");
        assert!(wiki_of_url("https://evil.example.org/wiki/X").is_none());
        assert!(wiki_of_url("http://en.wikipedia.org/wiki/X").is_none());
    }
}
