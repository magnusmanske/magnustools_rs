//! Common MediaWiki API calls; see [`wikimisc::wiki_api`], and the API client,
//! pages and titles from [`wikimisc::mediawiki`].

use crate::wiki::Wiki;
use anyhow::Result;
use std::collections::HashMap;

pub use wikimisc::mediawiki::{Api, MediaWikiError, Page, Title};
pub use wikimisc::wiki_api::*;

/// Titles per `action=query&titles=` request, the API limit for normal users.
const MAX_TITLES: usize = 50;

/// A page's wikitext and last revision.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PageContent {
    pub text: String,
    /// The last revision's timestamp (`2026-11-13T12:34:56Z`), for edit forms.
    pub timestamp: String,
    pub user: String,
}

/// The key that matches an API title to a requested one: spaces for
/// underscores, and the first letter in upper case.
pub(crate) fn title_key(title: &str) -> String {
    let title = title.trim().replace('_', " ");
    let mut chars = title.chars();
    chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
}

/// The wikitext of `titles` that exist, by [`title_key`]. One request per 50.
pub async fn page_contents(
    http: &reqwest::Client,
    wiki: &Wiki,
    titles: &[&str],
) -> Result<HashMap<String, PageContent>> {
    let mut pages = HashMap::new();
    for batch in titles.chunks(MAX_TITLES) {
        let titles = batch.join("|");
        let json = api(
            http,
            wiki,
            &[
                ("action", "query"),
                ("prop", "revisions"),
                ("rvprop", "content|timestamp|user"),
                ("rvslots", "main"),
                ("redirects", "1"),
                ("titles", titles.as_str()),
            ],
        )
        .await?;
        for page in json["query"]["pages"].as_array().into_iter().flatten() {
            let Some(title) = page["title"].as_str() else { continue };
            let revision = &page["revisions"][0];
            let text = revision["slots"]["main"]["content"]
                .as_str()
                .or_else(|| revision["content"].as_str())
                .unwrap_or_default()
                .to_string();
            let content = PageContent {
                text,
                timestamp: revision["timestamp"].as_str().unwrap_or_default().to_string(),
                user: revision["user"].as_str().unwrap_or_default().to_string(),
            };
            pages.insert(title_key(title), content);
        }
        // A redirect or a normalised title answers for the title that was asked
        // for, so callers can look pages up by the names they gave.
        for (key, field) in [("normalized", "from"), ("redirects", "from")] {
            let from = |item: &serde_json::Value| item[field].as_str().map(title_key);
            for item in json["query"][key].as_array().into_iter().flatten() {
                let (Some(from), Some(to)) = (from(item), item["to"].as_str().map(title_key)) else { continue };
                if let Some(content) = pages.get(&to).cloned() {
                    pages.entry(from).or_insert(content);
                }
            }
        }
    }
    Ok(pages)
}

/// The wikitext of one page, if it exists.
pub async fn page_content(http: &reqwest::Client, wiki: &Wiki, title: &str) -> Result<Option<PageContent>> {
    Ok(page_contents(http, wiki, &[title]).await?.remove(&title_key(title)))
}

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

    #[test]
    fn api_titles_match_requested_ones() {
        assert_eq!(title_key("File:some_name.jpg"), "File:some name.jpg");
        assert_eq!(title_key(" file:Some name.jpg "), "File:Some name.jpg");
        assert_eq!(title_key("some_name"), "Some name");
    }
}
