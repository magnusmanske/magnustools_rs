//! Common MediaWiki API calls; see [`wikimisc::wiki_api`].

use crate::wiki::Wiki;
use anyhow::Result;

pub use wikimisc::wiki_api::*;

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
}
