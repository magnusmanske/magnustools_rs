//! `dnb_link_finder.php`: Wikipedia pages that link to Google Books versions
//! of DNB volumes, which would better point to Wikisource.

use crate::mediawiki;
use crate::state::AppState;
use crate::web::{AppError, render};
use crate::wiki::Wiki;
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use wikimisc::mysql_async::Params as DbParams;
use wikimisc::mysql_async::Value as DbValue;
use wikimisc::mysql_async::prelude::Queryable;

const TITLE: &str = "DNB link finder";
const DATA_PAGE: &str = "User:Charles_Matthews/DNB_referencing_data";
/// Matching links shown; the list is for review, not exhaustive.
const MAX_LINKS: usize = 1_000;
/// The reversed-domain forms Google Books uses in `externallinks`.
const DOMAINS: &[&str] = &[
    "http://com.google.books.",
    "https://com.google.books.",
    "http://uk.co.google.books.",
    "https://uk.co.google.books.",
];

#[derive(Template)]
#[template(path = "tools/dnb_link_finder.html")]
struct Page {
    title: &'static str,
    ids: usize,
    links: Vec<Link>,
    truncated: bool,
}

struct Link {
    page_title: String,
    page_url: String,
    external_url: String,
    volume: String,
}

pub async fn handler(State(state): State<Arc<AppState>>) -> Result<Html<String>, AppError> {
    let wikisource = Wiki::new("en", "wikisource")?;
    let wikipedia = Wiki::new("en", "wikipedia")?;
    let wikitext = mediawiki::page_wikitext(&state.http, &wikisource, DATA_PAGE)
        .await?
        .ok_or_else(|| AppError::Upstream(anyhow::anyhow!("the DNB data page is missing")))?;
    let ids = parse_ids(&wikitext);
    let (links, truncated) = find_links(&state, &wikipedia, &ids).await?;
    render(&Page { title: TITLE, ids: ids.len(), links, truncated })
}

/// DNB IDs and their volume labels from the reference table.
fn parse_ids(wikitext: &str) -> HashMap<String, String> {
    let mut ids = HashMap::new();
    for line in wikitext.lines() {
        let line = line.trim();
        if line.starts_with("|-") || line == "|" || !line.starts_with('|') {
            continue;
        }
        if line.contains(['[', ']', '{', '}']) {
            continue;
        }
        let label = line[1..].trim().to_string();
        let Some(id) = line.split_whitespace().last() else {
            continue;
        };
        if id.chars().count() != 4 || id.contains('?') || id.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        ids.insert(id.to_string(), label);
    }
    ids
}

async fn find_links(
    state: &AppState,
    wikipedia: &Wiki,
    ids: &HashMap<String, String>,
) -> Result<(Vec<Link>, bool), AppError> {
    if ids.is_empty() {
        return Ok((Vec::new(), false));
    }
    let mut conn = state.replicas.conn(wikipedia, &["page", "externallinks"]).await?;
    let mut params = DOMAINS.iter().map(|domain| DbValue::from(*domain)).collect::<Vec<_>>();
    let ids_regex = ids.keys().map(|id| regex::escape(id)).collect::<Vec<_>>().join("|");
    params.push(DbValue::from(format!("id=({ids_regex})")));
    let sql = "SELECT p.page_title, e.el_to_domain_index, e.el_to_path
         FROM externallinks e JOIN page p ON e.el_from=p.page_id
         WHERE p.page_namespace=0 AND e.el_to_domain_index IN (?,?,?,?)
           AND e.el_to_path REGEXP ?";
    let rows: Vec<(String, String, Vec<u8>)> =
        conn.exec(sql, DbParams::Positional(params)).await.map_err(anyhow::Error::from)?;
    let mut seen = HashSet::new();
    let links: Vec<Link> = rows
        .into_iter()
        .filter_map(|(page_title, domain, path)| {
            let path = String::from_utf8_lossy(&path);
            let id = path.split_once("id=").map(|(_, rest)| rest.chars().take(4).collect::<String>())?;
            let volume = ids.get(&id)?;
            let external_url = external_url(&domain, &path);
            if !seen.insert((page_title.clone(), external_url.clone())) {
                return None;
            }
            Some(Link {
                page_url: wikipedia.page_url(&crate::replicas::display_title(&page_title)),
                page_title: crate::replicas::display_title(&page_title),
                external_url,
                volume: volume.clone(),
            })
        })
        .collect();
    let truncated = links.len() > MAX_LINKS;
    Ok((links.into_iter().take(MAX_LINKS).collect(), truncated))
}

/// The full URL from a reversed `externallinks` domain and its path.
fn external_url(domain_index: &str, path: &str) -> String {
    let (scheme, host) = domain_index.split_once("://").unwrap_or(("http", domain_index));
    let host = host.trim_end_matches('.').split('.').rev().collect::<Vec<_>>().join(".");
    format!("{scheme}://{host}{path}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_from_the_reference_table() {
        let text = "\
{|\n|-\n|Original DNB00\n|\n|\n|-\n|01 (1885) 4Lsc\n|[http://example.org]\n|\n|-\n|02 (1885) RysJ\n|\n|\n|}\n";
        let ids = parse_ids(text);
        assert_eq!(ids.get("4Lsc").map(String::as_str), Some("01 (1885) 4Lsc"));
        assert_eq!(ids.get("RysJ").map(String::as_str), Some("02 (1885) RysJ"));
        assert_eq!(ids.len(), 2);
    }

    #[test]
    fn full_url_from_reversed_domain() {
        assert_eq!(external_url("http://com.google.books.", "/books?id=4Lsc"), "http://books.google.com/books?id=4Lsc");
        assert_eq!(external_url("https://uk.co.google.books.", "/?id=RysJ"), "https://books.google.co.uk/?id=RysJ");
    }
}
