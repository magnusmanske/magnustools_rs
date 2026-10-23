//! `herding_sheep.php`: recent article edits by a group of users, given as
//! a category of user pages or as a list.

use crate::replicas::{db_title, display_title, select_in};
use crate::state::AppState;
use crate::web::{AppError, Params, render};
use crate::wiki::{Wiki, encode_component};
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use serde::Deserialize;
use std::sync::Arc;
use wikimisc::mysql_async::Value;

const NS_USER: i64 = 2;
const CATEGORY_DEPTH: u32 = 9;
const DEFAULT_LIMIT: u64 = 500;
const MAX_LIMIT: u64 = 5000;
const MAX_USERS: usize = 5000;

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default = "default_language")]
    language: String,
    #[serde(default = "default_project")]
    project: String,
    #[serde(default)]
    category: String,
    #[serde(default)]
    users: String,
    #[serde(default)]
    timestamp: String,
    #[serde(default)]
    timestamp_stop: String,
    limit: Option<String>,
    doit: Option<String>,
}

fn default_language() -> String {
    "en".to_string()
}

fn default_project() -> String {
    "wikipedia".to_string()
}

struct Edit {
    page: String,
    page_url: String,
    user: String,
    user_url: String,
    time: String,
}

struct Results {
    users: usize,
    category_url: Option<String>,
    edits: Vec<Edit>,
    per_user: Vec<(String, String, usize)>,
    next_url: Option<String>,
}

#[derive(Template)]
#[template(path = "tools/herding_sheep.html")]
struct Page<'a> {
    title: &'static str,
    request: &'a Request,
    limit: u64,
    results: Option<Results>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let limit =
        request.limit.as_deref().and_then(|l| l.trim().parse().ok()).unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let results = match request.doit.is_some() {
        true => Some(results(&state, &request, limit).await?),
        false => None,
    };
    render(&Page { title: "Herding sheep", request: &request, limit, results })
}

async fn results(state: &AppState, request: &Request, limit: u64) -> Result<Results, AppError> {
    let wiki = Wiki::new(&request.language, &request.project)?;
    let start = Bound::parse(&request.timestamp, '0')?;
    let stop = Bound::parse(&request.timestamp_stop, '9')?;
    let category = request.category.trim();
    let mut users: Vec<String> = match category.is_empty() {
        false => {
            let pages = state.replicas.pages_in_tree(&wiki, category, CATEGORY_DEPTH, NS_USER).await?;
            // User pages and their subpages
            pages.iter().map(|p| p.split('/').next().unwrap_or(p).to_string()).collect()
        }
        true => request.users.lines().map(db_title).filter(|u| !u.is_empty()).collect(),
    };
    users.sort();
    users.dedup();
    users.truncate(MAX_USERS);
    let names: Vec<String> = users.iter().map(|u| display_title(u)).collect();

    let mut conn = state.replicas.conn(&wiki, &["actor", "revision", "page"]).await?;
    let actors: Vec<u64> =
        select_in(&mut conn, "SELECT actor_id FROM actor WHERE actor_name IN", "", &[], &names).await?;
    let sql = "SELECT actor_name, page_title, rev_timestamp FROM revision_userindex
        JOIN page ON page_id=rev_page JOIN actor ON actor_id=rev_actor
        WHERE page_namespace=0 AND rev_timestamp>=? AND rev_timestamp<=? AND rev_actor IN";
    let suffix = format!("ORDER BY rev_timestamp DESC LIMIT {limit}");
    let params = [Value::from(start.0.as_str()), Value::from(stop.0.as_str())];
    let rows: Vec<(String, String, String)> = match actors.is_empty() {
        true => vec![],
        // One query, so that the limit and order hold; MAX_USERS keeps it to one chunk.
        false => select_in(&mut conn, sql, &suffix, &params, &actors).await?,
    };

    let mut per_user: Vec<(String, String, usize)> = vec![];
    let user_url = |name: &str| wiki.page_url(&format!("User:{name}"));
    let edits: Vec<Edit> = rows
        .iter()
        .map(|(user, page, timestamp)| {
            match per_user.iter_mut().find(|(u, ..)| u == user) {
                Some(entry) => entry.2 += 1,
                None => per_user.push((user.clone(), user_url(user), 1)),
            }
            Edit {
                page: display_title(page),
                page_url: wiki.page_url(page),
                user: user.clone(),
                user_url: user_url(user),
                time: format_timestamp(timestamp),
            }
        })
        .collect();
    let next_url = match rows.last() {
        Some((.., last)) if rows.len() as u64 == limit => Some(next_url(request, limit, last)),
        _ => None,
    };
    Ok(Results {
        users: names.len(),
        category_url: (!category.is_empty()).then(|| wiki.page_url(&format!("Category:{category}"))),
        edits,
        per_user,
        next_url,
    })
}

/// The next (older) page: up to just before the oldest edit shown.
fn next_url(request: &Request, limit: u64, oldest: &str) -> String {
    let before = oldest.parse::<u64>().map_or_else(|_| oldest.to_string(), |t| t.saturating_sub(1).to_string());
    let params = [
        ("language", request.language.as_str()),
        ("project", request.project.as_str()),
        ("category", request.category.as_str()),
        ("users", request.users.as_str()),
        ("timestamp", request.timestamp.as_str()),
        ("timestamp_stop", before.as_str()),
        ("limit", &limit.to_string()),
        ("doit", "1"),
    ];
    let query: Vec<String> = params.iter().map(|(k, v)| format!("{k}={}", encode_component(v))).collect();
    format!("herding_sheep.php?{}", query.join("&"))
}

/// A `YYYYMMDDHHMMSS` prefix, padded to a full timestamp.
struct Bound(String);

impl Bound {
    fn parse(prefix: &str, pad: char) -> Result<Self, AppError> {
        let prefix = prefix.trim();
        if prefix.len() > 14 || !prefix.chars().all(|c| c.is_ascii_digit()) {
            return Err(AppError::BadRequest(format!(
                "“{prefix}” is not a timestamp (YYYYMMDDHHMMSS, or the start of one)."
            )));
        }
        let padding: String = std::iter::repeat_n(pad, 14 - prefix.len()).collect();
        Ok(Self(format!("{prefix}{padding}")))
    }
}

/// `20240102030405` as `2024-01-02 03:04:05`.
fn format_timestamp(ts: &str) -> String {
    let part = |range: std::ops::Range<usize>| ts.get(range).unwrap_or_default();
    format!("{}-{}-{} {}:{}:{}", part(0..4), part(4..6), part(6..8), part(8..10), part(10..12), part(12..14))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds() {
        assert_eq!(Bound::parse("2024", '0').unwrap().0, "20240000000000");
        assert_eq!(Bound::parse(" 202403 ", '9').unwrap().0, "20240399999999");
        assert_eq!(Bound::parse("", '9').unwrap().0, "99999999999999");
        assert!(Bound::parse("2024' OR 1", '0').is_err());
        assert!(Bound::parse("202401010000000", '0').is_err());
    }

    #[test]
    fn timestamps() {
        assert_eq!(format_timestamp("20240102030405"), "2024-01-02 03:04:05");
    }

    #[test]
    fn next_page_ends_before_the_oldest_edit() {
        let request = Request {
            language: "de".into(),
            project: "wikipedia".into(),
            category: "Benutzer:X".into(),
            users: String::new(),
            timestamp: "2024".into(),
            timestamp_stop: String::new(),
            limit: None,
            doit: Some("1".into()),
        };
        assert_eq!(
            next_url(&request, 50, "20240102030405"),
            "herding_sheep.php?language=de&project=wikipedia&category=Benutzer%3AX&users=&timestamp=2024\
             &timestamp_stop=20240102030404&limit=50&doit=1"
        );
    }
}
