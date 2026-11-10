//! `random_out_of_sight.php`: a random selection of pages whose latest
//! revision has not been sighted, limited to small size changes (typo fixes
//! and the like).

use super::sighting;
use crate::state::AppState;
use crate::web::{AppError, Params, render};
use crate::wiki::Wiki;
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use serde::Deserialize;
use std::sync::Arc;
use wikimisc::mysql_async::prelude::Queryable;

const TITLE: &str = "Random out-of-sight";
const DEFAULT_OUT: u64 = 20;
const MAX_OUT: u64 = 100;
const DEFAULT_LIMIT: u64 = 5;
const MAX_LIMIT: u64 = 100_000;

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Request {
    language: String,
    project: String,
    out: Option<String>,
    limit: Option<String>,
    doit: Option<String>,
}

impl Default for Request {
    fn default() -> Self {
        Self {
            language: sighting::default_language(),
            project: sighting::default_project(),
            out: None,
            limit: None,
            doit: None,
        }
    }
}

struct PageRow {
    title: String,
    url: String,
    diff: i64,
}

impl PageRow {
    fn diff_text(&self) -> String {
        match self.diff {
            0 => "±0".to_string(),
            diff if diff > 0 => format!("+{diff}"),
            diff => diff.to_string(),
        }
    }

    fn diff_class(&self) -> &'static str {
        match self.diff {
            diff if diff > 0 => "diff-plus",
            diff if diff < 0 => "diff-minus",
            _ => "",
        }
    }
}

struct Results {
    pages: Vec<PageRow>,
}

#[derive(Template)]
#[template(path = "tools/random_out_of_sight.html")]
struct Page<'a> {
    title: &'static str,
    request: &'a Request,
    out: u64,
    limit: u64,
    results: Option<Results>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let out = number(request.out.as_deref(), DEFAULT_OUT, 1, MAX_OUT);
    let limit = number(request.limit.as_deref(), DEFAULT_LIMIT, 0, MAX_LIMIT);
    let wiki = Wiki::new(&request.language, &request.project)?;
    let run = request.doit.is_some() || request.out.is_some() || request.limit.is_some();
    let results = match run {
        true => Some(results(&state, &wiki, out, limit).await?),
        false => None,
    };
    render(&Page { title: TITLE, request: &request, out, limit, results })
}

async fn results(state: &AppState, wiki: &Wiki, out: u64, limit: u64) -> Result<Results, AppError> {
    let mut conn = state.replicas.conn(wiki, &["page", "flaggedpages", "revision"]).await?;
    let sql = "SELECT p.page_title, p.page_len, r.rev_len, fp.fp_stable
         FROM page p
         JOIN flaggedpages fp ON fp.fp_page_id=p.page_id
         JOIN revision r ON r.rev_id=fp.fp_stable
         WHERE p.page_namespace=0 AND p.page_latest<>fp.fp_stable
           AND (CAST(p.page_len AS SIGNED) - CAST(r.rev_len AS SIGNED)) BETWEEN ? AND ?
         ORDER BY RAND()
         LIMIT ?";
    let limit = limit as i64;
    let rows: Vec<(String, u64, Option<u64>, u64)> =
        conn.exec(sql, (-limit, limit, out)).await.map_err(anyhow::Error::from)?;
    let pages = rows
        .into_iter()
        .map(|(title, page_len, rev_len, stable)| PageRow {
            title: crate::replicas::display_title(&title),
            url: sighting::diff_url(wiki, &title, stable),
            diff: page_len as i64 - rev_len.unwrap_or(page_len) as i64,
        })
        .collect();
    Ok(Results { pages })
}

/// A form number, clamped to `min..=max`; invalid input falls back.
fn number(value: Option<&str>, default: u64, min: u64, max: u64) -> u64 {
    value.and_then(|value| value.trim().parse().ok()).unwrap_or(default).clamp(min, max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;

    #[test]
    fn numbers_fall_back_and_are_capped() {
        assert_eq!(number(None, 20, 1, 100), 20);
        assert_eq!(number(Some("7"), 20, 1, 100), 7);
        assert_eq!(number(Some("nope"), 20, 1, 100), 20);
        assert_eq!(number(Some("0"), 20, 1, 100), 1);
        assert_eq!(number(Some("999"), 20, 1, 100), 100);
        assert_eq!(number(Some("0"), 5, 0, 100), 0);
    }

    #[tokio::test]
    async fn form_renders() {
        let dir = tempfile::tempdir().unwrap();
        let page = get(dir.path(), "/random_out_of_sight/").await;
        assert_eq!(page.status.as_u16(), 200);
        assert!(page.body.contains("<h1>Random out-of-sight</h1>"));
        assert!(page.body.contains("name=\"limit\""));
    }
}
