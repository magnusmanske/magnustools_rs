//! Toolhub and Wikidata enrichment.

use crate::state::AppState;
use anyhow::Result;
use serde_json::Value;
use std::collections::HashSet;
use wikimisc::mysql_async::Conn;
use wikimisc::mysql_async::prelude::Queryable;

const TOOLHUB_SEARCH: &str = "https://toolhub.wikimedia.org/api/search/tools/?format=json&ordering=-score&page=1&page_size=1000&q=%22Magnus+Manske%22";

/// Links tools to their Toolhub entries, and takes descriptions (where
/// missing) and keywords from there.
pub async fn toolhub(state: &AppState, conn: &mut Conn) -> Result<()> {
    let known: HashSet<String> =
        conn.query("SELECT DISTINCT toolhub FROM tool WHERE toolhub!=''").await?.into_iter().collect();
    let json: Value = state.http.get(TOOLHUB_SEARCH).send().await?.error_for_status()?.json().await?;
    for entry in json["results"].as_array().into_iter().flatten() {
        let text = |key: &str| entry[key].as_str().unwrap_or_default().trim().to_string();
        let name = text("name");
        if name.is_empty() {
            continue;
        }
        if !known.contains(&name) {
            let title = text("title");
            let url = text("url").replace("http:", "https:");
            let candidates: Vec<u64> = conn
                .exec(
                    "SELECT id FROM tool WHERE toolhub='' AND (url=? OR name IN (?, ?, ?, ?))",
                    (
                        url.as_str(),
                        title.as_str(),
                        title.replace(' ', "_"),
                        title.replace(' ', "-"),
                        title.replace(' ', ""),
                    ),
                )
                .await?;
            match candidates.as_slice() {
                [id] => conn.exec_drop("UPDATE tool SET toolhub=? WHERE id=?", (&name, id)).await?,
                [] => {
                    tracing::info!("Toolhub tool not found here: {title} / {name}: {url}");
                    continue;
                }
                _ => {
                    tracing::info!("Toolhub tool matches more than one tool: {title} / {name}: {url}");
                    continue;
                }
            }
        }
        let description = text("description");
        if !description.is_empty() {
            conn.exec_drop("UPDATE tool SET description=? WHERE description='' AND toolhub=?", (&description, &name))
                .await?;
        }
        for keyword in entry["keywords"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            conn.exec_drop(
                "INSERT IGNORE INTO keywords (tool_id, keyword) SELECT id, ? FROM tool WHERE toolhub=?",
                (keyword.trim(), &name),
            )
            .await?;
        }
    }
    Ok(())
}

/// Reports Wikidata items of Magnus' tools that no tool here points to.
pub(super) async fn wikidata_tool_items(state: &AppState, conn: &mut Conn) -> Result<()> {
    let known: HashSet<String> =
        conn.query("SELECT value FROM tool_kv WHERE `key`='item'").await?.into_iter().collect();
    let query = "SELECT ?q { ?q wdt:P31 wd:Q20726407 ; wdt:P178 wd:Q13520818 }";
    let json: Value = state
        .http
        .get("https://query.wikidata.org/sparql")
        .query(&[("query", query), ("format", "json")])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    for binding in json["results"]["bindings"].as_array().into_iter().flatten() {
        let q = binding["q"]["value"].as_str().and_then(|u| u.rsplit('/').next()).unwrap_or_default();
        if !q.is_empty() && !known.contains(q) {
            tracing::info!("No key/value pair for tool https://www.wikidata.org/wiki/{q}");
        }
    }
    Ok(())
}
