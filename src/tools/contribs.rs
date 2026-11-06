//! `contribs.php`: the recent contributions of several users, interleaved
//! by time, so bursts of edits by different accounts stand out.

use crate::mediawiki;
use crate::replicas::{display_title, format_timestamp, select_in};
use crate::state::AppState;
use crate::web::{AppError, Params, render};
use crate::wiki::{Wiki, upper_first};
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use chrono::NaiveDateTime;
use serde::Deserialize;
use std::sync::Arc;

const DEFAULT_LIMIT: u64 = 100;
const MAX_LIMIT: u64 = 500;
const MAX_USERS: usize = 100;
const BURST_SECONDS: i64 = 60;

/// `rev_actor`, namespace, title, revision ID, timestamp, minor, bytes, comment, latest.
type RevisionRow = (String, i64, String, u64, String, bool, Option<u32>, Option<String>, u64);

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default = "default_language", alias = "language")]
    lang: String,
    #[serde(default = "default_project", alias = "project")]
    family: String,
    #[serde(default)]
    users: String,
    #[serde(default)]
    order: String,
    limit: Option<String>,
}

fn default_language() -> String {
    "en".to_string()
}

fn default_project() -> String {
    "wikipedia".to_string()
}

struct Edit {
    time: String,
    user: String,
    user_url: String,
    page: String,
    page_url: String,
    hist_url: String,
    diff_url: String,
    contribs_url: String,
    comment: String,
    bytes: Option<u32>,
    top: bool,
    new_user: bool,
    gap: Option<String>,
}

struct Results {
    edits: Vec<Edit>,
    missing: Vec<String>,
    wiki: String,
}

#[derive(Template)]
#[template(path = "tools/contribs.html")]
struct Page<'a> {
    title: &'static str,
    request: &'a Request,
    limit: u64,
    oldest_first: bool,
    results: Option<Results>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let limit =
        request.limit.as_deref().and_then(|l| l.trim().parse().ok()).unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let oldest_first = request.order.eq_ignore_ascii_case("ASC");
    let results = match users(&request.users).is_empty() {
        true => None,
        false => Some(results(&state, &request, limit, oldest_first).await?),
    };
    render(&Page { title: "Contribs [Erwin85]", request: &request, limit, oldest_first, results })
}

/// The user names, as the wiki has them; empty ones and repeats left out.
fn users(input: &str) -> Vec<String> {
    let mut users: Vec<String> = input
        .split(['|', '\n', ','])
        .map(|u| upper_first(&u.trim().replace('_', " ")))
        .filter(|u| !u.is_empty())
        .take(MAX_USERS)
        .collect();
    let mut seen = std::collections::HashSet::new();
    users.retain(|u| seen.insert(u.clone()));
    users
}

async fn results(state: &AppState, request: &Request, limit: u64, oldest_first: bool) -> Result<Results, AppError> {
    let wiki = Wiki::new(&request.lang, &request.family)?;
    let names = users(&request.users);
    let mut conn = state.replicas.conn(&wiki, &["actor", "revision", "page", "comment"]).await?;
    let actors: Vec<(u64, String)> =
        select_in(&mut conn, "SELECT actor_id, actor_name FROM actor WHERE actor_name IN", "", &[], &names).await?;
    if actors.is_empty() {
        return Ok(Results { edits: vec![], missing: names, wiki: wiki.to_string() });
    }
    let ids: Vec<u64> = actors.iter().map(|(id, _)| *id).collect();
    let order = if oldest_first { "ASC" } else { "DESC" };
    let sql = "SELECT a.actor_name, p.page_namespace, p.page_title, r.rev_id, r.rev_timestamp,
            r.rev_minor_edit, r.rev_len, c.comment_text, p.page_latest
        FROM revision_userindex r JOIN page p ON p.page_id=r.rev_page
        JOIN actor a ON a.actor_id=r.rev_actor
        LEFT JOIN comment c ON c.comment_id=r.rev_comment_id
        WHERE r.rev_actor IN";
    let suffix = format!("ORDER BY r.rev_timestamp {order} LIMIT {limit}");
    let rows: Vec<RevisionRow> = select_in(&mut conn, sql, &suffix, &[], &ids).await?;

    let found: std::collections::HashSet<String> = actors.into_iter().map(|(_, name)| name).collect();
    let missing: Vec<String> = names.into_iter().filter(|name| !found.contains(name)).collect();
    let namespace_names = mediawiki::namespace_names(&state.http, &wiki).await.unwrap_or_default();
    let mut edits: Vec<Edit> = Vec::with_capacity(rows.len());
    let mut previous_time: Option<NaiveDateTime> = None;
    for (user, namespace, title, rev_id, timestamp, minor, bytes, comment, latest) in rows {
        let full = mediawiki::title_with_namespace(namespace, &display_title(&title), &namespace_names);
        let parsed = NaiveDateTime::parse_from_str(&timestamp, "%Y%m%d%H%M%S").ok();
        let gap = match (previous_time, parsed) {
            (Some(previous), Some(current)) => {
                let seconds = (current - previous).num_seconds().abs();
                (seconds < BURST_SECONDS).then(|| format!("{seconds} s"))
            }
            _ => None,
        };
        previous_time = parsed;
        let encoded = crate::wiki::encode_title(&full);
        let comment = match (minor, comment.unwrap_or_default()) {
            (true, comment) if comment.is_empty() => "(minor)".to_string(),
            (true, comment) => format!("{comment} (minor)"),
            (false, comment) => comment,
        };
        edits.push(Edit {
            time: format_timestamp(&timestamp),
            user: user.clone(),
            user_url: wiki.page_url(&format!("User:{user}")),
            page: full.clone(),
            page_url: wiki.page_url(&full),
            hist_url: format!("{}?title={encoded}&action=history", wiki.index_url()),
            diff_url: format!("{}?title={encoded}&diff=prev&oldid={rev_id}", wiki.index_url()),
            contribs_url: wiki.page_url(&format!("Special:Contributions/{user}")),
            comment,
            bytes,
            top: latest == rev_id,
            new_user: edits.last().is_none_or(|edit| edit.user != user),
            gap,
        });
    }
    Ok(Results { edits, missing, wiki: wiki.to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_lists() {
        assert_eq!(users("A|B\nC,D|A"), vec!["A", "B", "C", "D"]);
        assert_eq!(users(" A_B , , C "), vec!["A B", "C"]);
    }

    #[test]
    fn timestamps_parse() {
        assert!(NaiveDateTime::parse_from_str("20240102030405", "%Y%m%d%H%M%S").is_ok());
        assert!(NaiveDateTime::parse_from_str("infinity", "%Y%m%d%H%M%S").is_err());
    }
}
