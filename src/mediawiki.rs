//! Common MediaWiki API calls; see [`wikimisc::wiki_api`].

use crate::cache::TimedMap;
use crate::wiki::Wiki;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

pub use wikimisc::wiki_api::*;

/// Namespace names per wiki, which hardly ever change.
static NAMESPACES: TimedMap<String, HashMap<i64, String>> = TimedMap::new(Duration::from_secs(86_400), 1000);

/// Titles per `action=query&titles=` request, the API limit for normal users.
const MAX_TITLES: usize = 50;

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

/// Namespace names by ID, for a wiki's pages.
pub async fn namespace_names(http: &reqwest::Client, wiki: &Wiki) -> Result<Arc<HashMap<i64, String>>> {
    NAMESPACES.get_or_fetch(wiki.dbname().to_string(), async || fetch_namespace_names(http, wiki).await).await
}

async fn fetch_namespace_names(http: &reqwest::Client, wiki: &Wiki) -> Result<HashMap<i64, String>> {
    let json = api(http, wiki, &[("action", "query"), ("meta", "siteinfo"), ("siprop", "namespaces")]).await?;
    Ok(json["query"]["namespaces"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(_, ns)| {
            let name = ns["name"].as_str().filter(|n| !n.is_empty()).unwrap_or("Article");
            Some((ns["id"].as_i64()?, name.to_string()))
        })
        .collect())
}

/// `title` as it appears on the wiki, with the namespace name (if any).
pub fn title_with_namespace(namespace: i64, title: &str, names: &HashMap<i64, String>) -> String {
    match names.get(&namespace).filter(|_| namespace != 0) {
        Some(name) => format!("{name}:{title}"),
        None => title.to_string(),
    }
}

/// An API title as the database has it: without namespace prefix, with underscores.
pub fn database_title(namespace: i64, title: &str) -> String {
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

    #[test]
    fn titles_with_namespaces() {
        let names = HashMap::from([(0, "Article".to_string()), (2, "User".to_string())]);
        assert_eq!(title_with_namespace(0, "A_b", &names), "A_b");
        assert_eq!(title_with_namespace(2, "A_b", &names), "User:A_b");
        assert_eq!(title_with_namespace(4, "A_b", &names), "A_b");
    }
}
