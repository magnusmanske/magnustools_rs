//! The main page: all active tools, grouped by tag, from the tool database.
//! (Formerly `main.html`, written nightly by the buggregator job.)

use crate::cache::TimedCache;
use crate::state::AppState;
use crate::web::{AppError, render, thousands};
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use chrono::{Datelike, Months, NaiveDate, Utc};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use wikimisc::mysql_async::prelude::Queryable;

const CACHE_FOR: Duration = Duration::from_secs(600);
const POPULAR_TAG: &str = "popular";
/// Uses per day, on average over last month, to count as "most used".
const MOST_USED_PER_DAY: u64 = 50;

struct Tool {
    name: String,
    url: Option<String>,
    description: String,
    note: String,
    repo: Option<String>,
    toolhub: Option<String>,
    item: Option<String>,
    no_webservice: bool,
    /// For "most used": uses last month.
    usage: Option<String>,
}

struct Group {
    key: String,
    name: String,
    note: Option<&'static str>,
    tools: Vec<Tool>,
}

struct MainPage {
    tool_count: usize,
    groups: Vec<Group>,
}

#[derive(Template)]
#[template(path = "tools/index.html")]
struct Page {
    title: &'static str,
    main: Arc<MainPage>,
}

static CACHE: TimedCache<MainPage> = TimedCache::new(CACHE_FOR);

pub async fn handler(State(state): State<Arc<AppState>>) -> Result<Html<String>, AppError> {
    let main = CACHE.get_or_fetch(async || load(&state).await).await?;
    render(&Page { title: "Magnus' tools", main })
}

/// A row of the `tool` table.
type ToolRow = (u32, String, String, String, String, String, String, bool);

async fn load(state: &AppState) -> anyhow::Result<MainPage> {
    let mut conn = state.tool_db.conn().await?;
    let rows: Vec<ToolRow> = conn
        .query(
            "SELECT id, name, subdomain, description, note, url, toolhub, has_webservice FROM tool WHERE is_active=1",
        )
        .await?;
    let repos: HashMap<u32, String> = conn.query("SELECT tool_id, html_url FROM git_repo").await?.into_iter().collect();
    let items: HashMap<u32, String> =
        conn.query("SELECT tool_id, value FROM tool_kv WHERE `key`='item'").await?.into_iter().collect();
    let tags: Vec<(u32, String)> = conn.query("SELECT id, tag FROM tag ORDER BY id").await?;
    let tagged: Vec<(u32, u32)> = conn.query("SELECT tag_id, tool_id FROM tag2tool").await?;
    let (first, last) = last_month();
    let most_used: Vec<(u32, u64)> = conn
        .exec(
            "SELECT lt.main_tool_id, SUM(u.used) AS uses FROM logging_uses u JOIN logging_tools lt ON lt.id=u.tool_id
             WHERE u.date BETWEEN ? AND ? AND lt.main_tool_id IS NOT NULL
             GROUP BY lt.main_tool_id HAVING uses >= ? ORDER BY uses DESC",
            (first, last, MOST_USED_PER_DAY * 30),
        )
        .await?;

    let tools: HashMap<u32, ToolRow> = rows.into_iter().map(|row| (row.0, row)).collect();
    let tool = |id: u32, usage: Option<u64>| -> Option<Tool> {
        let (_, name, subdomain, description, note, url, toolhub, has_webservice) = tools.get(&id)?;
        let name = match name.as_str() {
            "SUGGESTED TOOL" => return None,
            "WHOLE TOOL" => format!("[Wrapper entry for {subdomain}]"),
            _ => ucfirst(name.replace('_', " ").trim()),
        };
        let on_a_wiki = ["wikidata.org", "wikipedia.org", "commons.wikimedia.org"].iter().any(|w| url.contains(w));
        Some(Tool {
            name,
            url: non_empty(url),
            description: description.trim().to_string(),
            note: note.trim().to_string(),
            repo: repos.get(&id).cloned(),
            toolhub: non_empty(toolhub).map(|t| format!("https://toolhub.wikimedia.org/tools/{t}")),
            item: items.get(&id).map(|q| format!("https://www.wikidata.org/wiki/{q}")),
            no_webservice: !has_webservice && !on_a_wiki,
            usage: usage.map(thousands),
        })
    };
    let mut seen: HashSet<u32> = HashSet::new();
    let mut tagged_tools = |tag_id: u32| -> Vec<Tool> {
        let mut list: Vec<Tool> = tagged
            .iter()
            .filter(|(tag, _)| *tag == tag_id)
            .filter_map(|(_, id)| {
                seen.insert(*id);
                tool(*id, None)
            })
            .collect();
        list.sort_by_key(|t| t.name.to_lowercase());
        list
    };

    let mut groups = vec![];
    let popular = tags.iter().find(|(_, tag)| tag == POPULAR_TAG).map(|(id, _)| *id);
    if let Some(popular) = popular {
        groups.push(group("Popular tools", None, tagged_tools(popular)));
    }
    let most_used: Vec<Tool> = most_used.iter().filter_map(|(id, uses)| tool(*id, Some(*uses))).collect();
    groups.push(group("Most used tools", Some("Many tools do not collect central usage figures."), most_used));
    for (id, tag) in tags.iter().filter(|(id, _)| Some(*id) != popular) {
        groups.push(group(&ucfirst(tag), None, tagged_tools(*id)));
    }
    let mut ids: Vec<u32> = tools.keys().filter(|id| !seen.contains(id)).copied().collect();
    ids.sort();
    let mut other: Vec<Tool> = ids.into_iter().filter_map(|id| tool(id, None)).collect();
    other.sort_by_key(|t| t.name.to_lowercase());
    groups.push(group("Other", None, other));
    groups.retain(|g| !g.tools.is_empty());
    Ok(MainPage { tool_count: tools.len(), groups })
}

fn group(name: &str, note: Option<&'static str>, tools: Vec<Tool>) -> Group {
    Group { key: name.to_lowercase().replace(' ', "_"), name: name.to_string(), note, tools }
}

/// First and last day of last month, as the log stores days (`YYYYMMDD`).
fn last_month() -> (u32, u32) {
    let today = Utc::now().date_naive();
    let this_month = today.with_day(1).unwrap_or(today);
    let first = this_month.checked_sub_months(Months::new(1)).unwrap_or(this_month);
    let last = this_month.pred_opt().unwrap_or(this_month);
    (day_number(first), day_number(last))
}

fn day_number(date: NaiveDate) -> u32 {
    date.year().unsigned_abs() * 10_000 + date.month() * 100 + date.day()
}

fn non_empty(s: &str) -> Option<String> {
    Some(s.trim().to_string()).filter(|s| !s.is_empty())
}

fn ucfirst(s: &str) -> String {
    let mut chars = s.chars();
    chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days() {
        assert_eq!(day_number(NaiveDate::from_ymd_opt(2026, 9, 30).unwrap()), 20260930);
        let (first, last) = last_month();
        assert!(first % 100 == 1 && last > first && last - first < 31);
    }
}
