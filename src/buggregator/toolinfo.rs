//! `toolinfo_buggregator.json`: the tools that are not on Toolhub yet, in
//! the toolinfo format Toolhub crawls. (Formerly written by a nightly job.)

use crate::cache::TimedCache;
use crate::state::AppState;
use crate::web::AppError;
use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use wikimisc::mysql_async::prelude::Queryable;

const CACHE_FOR: Duration = Duration::from_secs(3600);
/// A tool whose note says one of these needs a status in `tool_kv`, not a Toolhub entry.
const BAD_TAGS: &[&str] = &["deactivated", "replaced", "no results", "inactive", "broken", "planned", "error", "fixme"];
const KEYWORD_SITES: &[&str] = &["wikipedia", "wikisource", "wikibooks", "wikidata", "commons"];

#[derive(Debug, Serialize)]
struct ToolInfo {
    name: String,
    title: String,
    description: String,
    url: String,
    keywords: String,
    author: &'static str,
    repository: String,
}

static CACHE: TimedCache<String> = TimedCache::new(CACHE_FOR);

pub async fn handler(State(state): State<Arc<AppState>>) -> Result<Response, AppError> {
    let json = CACHE
        .get_or_fetch(async || {
            let tools = load(&state).await?;
            Ok::<_, anyhow::Error>(serde_json::to_string_pretty(&tools)?)
        })
        .await?;
    Ok(([(header::CONTENT_TYPE, "application/json")], json.as_str().to_owned()).into_response())
}

type Row = (u64, String, String, String, String, String);

async fn load(state: &AppState) -> anyhow::Result<Vec<ToolInfo>> {
    let mut conn = state.tool_db.conn().await?;
    let repos: BTreeMap<u64, String> =
        conn.query("SELECT tool_id, html_url FROM git_repo").await?.into_iter().collect();
    let mut keywords: BTreeMap<u64, Vec<String>> = BTreeMap::new();
    for (tool_id, keyword) in conn.query::<(u64, String), _>("SELECT tool_id, keyword FROM keywords").await? {
        keywords.entry(tool_id).or_default().push(keyword);
    }
    let rows: Vec<Row> = conn
        .query(
            "SELECT id, name, subdomain, description, note, url FROM tool
             WHERE toolhub='' AND NOT EXISTS (SELECT * FROM tool_kv WHERE tool_id=tool.id AND `key`='status')",
        )
        .await?;
    let mut names: Vec<String> = vec![];
    let mut tools = vec![];
    for (id, name, subdomain, description, note, url) in rows {
        let lower_note = note.to_lowercase();
        if name == "WHOLE TOOL" || name == "index" || url.is_empty() || BAD_TAGS.iter().any(|t| lower_note.contains(t))
        {
            continue;
        }
        let description = if description.is_empty() { note.clone() } else { description };
        if description.is_empty() {
            continue;
        }
        let mut toolinfo_name: String = format!("mm_{name}")
            .to_lowercase()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || "_-".contains(*c))
            .collect();
        if names.contains(&toolinfo_name) {
            toolinfo_name = format!("{toolinfo_name}_{subdomain}");
        }
        names.push(toolinfo_name.clone());

        let mut words: Vec<String> = keywords.remove(&id).unwrap_or_default();
        if lower_note.contains("javascript") {
            words.push("javascript".to_string());
        }
        for site in KEYWORD_SITES {
            if [&lower_note, &name.to_lowercase(), &subdomain.to_lowercase()].iter().any(|s| s.contains(site)) {
                words.push(site.to_string());
            }
        }
        let mut seen = std::collections::HashSet::new();
        words.retain(|w| seen.insert(w.clone()));
        tools.push(ToolInfo {
            name: toolinfo_name,
            title: name.replace('_', " "),
            description,
            url,
            keywords: words.join(", "),
            author: "Magnus Manske",
            repository: repos.get(&id).cloned().unwrap_or_default(),
        });
    }
    Ok(tools)
}
