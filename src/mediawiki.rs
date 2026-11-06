//! Common MediaWiki API calls; see [`wikimisc::wiki_api`], and the API client
//! and titles from [`wikimisc::mediawiki`].

use crate::wiki::Wiki;
use anyhow::Result;
use std::collections::HashMap;

pub use wikimisc::mediawiki::{Api, Title};
pub use wikimisc::wiki_api::*;

/// Titles per `action=query&titles=` request, the API limit for normal users.
const MAX_TITLES: usize = 50;

/// Namespace names by ID, from the API's cached site info.
pub fn namespace_names(api: &Api) -> HashMap<i64, String> {
    api.get_site_info()["query"]["namespaces"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(_, ns)| {
            let name = ns["*"]
                .as_str()
                .or_else(|| ns["canonical"].as_str())
                .filter(|name| !name.is_empty())
                .unwrap_or("Article");
            Some((ns["id"].as_i64()?, name.to_string()))
        })
        .collect()
}

/// `title` without a leading namespace from `names` (case-insensitive).
///
/// This is the stricter parser from [`wikimisc::title`], replacing the one in
/// [`wikimisc::wiki_api`].
pub fn strip_namespace<'a, S: AsRef<str>>(title: &'a str, names: &[S]) -> Option<&'a str> {
    wikimisc::title::strip_namespace(title, names)
}

/// `(page ID, namespace, database title)` of the pages that exist; the API
/// knows the wiki's namespace names.
pub async fn resolve_pages(http: &reqwest::Client, wiki: &Wiki, titles: &[&str]) -> Result<Vec<(u64, i64, String)>> {
    let mut pages = vec![];
    for batch in titles.chunks(MAX_TITLES) {
        let titles = batch.join("|");
        let json = api(http, wiki, &[("action", "query"), ("titles", titles.as_str())]).await?;
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
pub fn database_title(namespace: i64, title: &str) -> String {
    let title = match namespace {
        0 => title,
        _ => title.split_once(':').map_or(title, |(_, rest)| rest),
    };
    wikimisc::title::to_db_key(title)
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
