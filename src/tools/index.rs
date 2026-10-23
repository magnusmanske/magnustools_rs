//! The main page: all active tools, grouped by tag, from the tool database.
//! (Formerly `main.html`, written nightly by the buggregator job.)

use crate::cache::TimedCache;
use crate::state::AppState;
use crate::web::{AppError, render, thousands};
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use chrono::{Datelike, Days, NaiveDate, Utc};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use wikimisc::mysql_async::Conn;
use wikimisc::mysql_async::prelude::Queryable;

const CACHE_FOR: Duration = Duration::from_secs(600);
const POPULAR_TAG: &str = "popular";
/// Uses per day, on average over the last `USAGE_DAYS` days, to count as "most used".
const MOST_USED_PER_DAY: u64 = 50;
const USAGE_DAYS: u64 = 30;

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
    let most_used = most_used(&mut conn).await?;

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

/// Uses per tool over the last days, most first, of the tools used enough:
/// uses counted by the logger, plus successful requests to the services that
/// report to the dashboard. (Tools move from the one to the other, so for
/// the same tool the two count different periods.)
async fn most_used(conn: &mut Conn) -> anyhow::Result<Vec<(u32, u64)>> {
    let today = Utc::now().date_naive();
    let first = today.checked_sub_days(Days::new(USAGE_DAYS)).unwrap_or(today);
    let logged: Vec<(u32, u64)> = conn
        .exec(
            "SELECT lt.main_tool_id, SUM(u.used) FROM logging_uses u JOIN logging_tools lt ON lt.id=u.tool_id
             WHERE u.date BETWEEN ? AND ? AND lt.main_tool_id IS NOT NULL GROUP BY lt.main_tool_id",
            (day_number(first), day_number(today.pred_opt().unwrap_or(today))),
        )
        .await?;
    let served: Vec<(u32, f64)> = conn
        .exec(
            "SELECT ds.tool_id, COALESCE(SUM(h.delta_sum), 0) FROM dash_service ds
             JOIN dash_series s ON s.service_id=ds.id JOIN dash_point_hourly h ON h.series_id=s.id
             WHERE ds.tool_id IS NOT NULL AND s.name='http_requests_total' AND JSON_VALUE(s.labels, '$.status')='2xx'
             AND h.hour >= ? AND h.hour < ? GROUP BY ds.tool_id",
            (unix_start(first), unix_start(today)),
        )
        .await?;
    Ok(combine(logged, served))
}

/// The two counts added up per tool; those under the threshold left out.
fn combine(logged: Vec<(u32, u64)>, served: Vec<(u32, f64)>) -> Vec<(u32, u64)> {
    let mut uses: HashMap<u32, u64> = logged.into_iter().collect();
    for (tool, requests) in served {
        // Counters are whole numbers, stored as floating point.
        let requests = requests.max(0.0).round() as u64;
        *uses.entry(tool).or_default() += requests;
    }
    let mut uses: Vec<(u32, u64)> = uses.into_iter().filter(|(_, n)| *n >= MOST_USED_PER_DAY * USAGE_DAYS).collect();
    uses.sort_by(|(a, x), (b, y)| y.cmp(x).then(a.cmp(b)));
    uses
}

/// Midnight (UTC) at the start of `date`, in Unix seconds.
fn unix_start(date: NaiveDate) -> i64 {
    date.and_hms_opt(0, 0, 0).map_or(0, |t| t.and_utc().timestamp())
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
        let date = NaiveDate::from_ymd_opt(2026, 9, 30).unwrap();
        assert_eq!(day_number(date), 20260930);
        assert_eq!(unix_start(date), 1_790_726_400);
    }

    #[test]
    fn counts_added_up() {
        let threshold = MOST_USED_PER_DAY * USAGE_DAYS;
        let logged = vec![(1, threshold - 10), (2, 5), (3, threshold * 2)];
        let served = vec![(1, 10.0), (4, threshold as f64 * 3.0), (5, 1.0)];
        assert_eq!(combine(logged, served), vec![(4, threshold * 3), (3, threshold * 2), (1, threshold)]);
    }
}
