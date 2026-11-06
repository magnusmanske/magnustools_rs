//! `subpages.php`: the subpages of a page, for a raw watchlist.

use crate::mediawiki;
use crate::state::AppState;
use crate::web::{AppError, Params, render};
use crate::wiki::Wiki;
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use serde::Deserialize;
use std::sync::Arc;
use wikimisc::mysql_async::prelude::Queryable;

const MAX_SUBPAGES: u64 = 10_000;

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default = "default_language", alias = "language")]
    lang: String,
    #[serde(default = "default_project", alias = "project")]
    family: String,
    #[serde(default)]
    title: String,
    #[serde(default, alias = "namespace")]
    namespaces: String,
}

fn default_language() -> String {
    "en".to_string()
}

fn default_project() -> String {
    "wikipedia".to_string()
}

struct Results {
    parent: String,
    subpages: String,
    count: usize,
    truncated: bool,
}

#[derive(Template)]
#[template(path = "tools/subpages.html")]
struct Page<'a> {
    title: &'static str,
    request: &'a Request,
    results: Option<Results>,
    error: Option<String>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let mut page = Page { title: "Subpages [Erwin85]", request: &request, results: None, error: None };
    if request.title.trim().is_empty() {
        return render(&page);
    }
    let namespace = match request.namespaces.trim() {
        "" => 0,
        value => match value.parse() {
            Ok(namespace) => namespace,
            Err(_) => {
                page.error = Some(format!("“{value}” is not a namespace ID."));
                return render(&page);
            }
        },
    };
    page.results = Some(results(&state, &request, namespace).await?);
    render(&page)
}

async fn results(state: &AppState, request: &Request, selected: i64) -> Result<Results, AppError> {
    let wiki = Wiki::new(&request.lang, &request.family)?;
    let api = state.api(&wiki).await?;
    let parsed = mediawiki::Title::new_from_full(&request.title, &api);
    let namespace = match (selected, parsed.namespace_id()) {
        (-1, 0) => -1,
        (-1, namespace) => namespace,
        (_, namespace) if namespace != 0 => namespace,
        (namespace, _) => namespace,
    };
    let mut conn = state.replicas.conn(&wiki, &["page"]).await?;
    let title = parsed.with_underscores();
    let pattern = format!("{}/%", like_escape(&title));
    let namespace_condition = if namespace == -1 { "" } else { "AND page_namespace = ?" };
    let sql = format!(
        "SELECT page_namespace, page_title FROM page
         WHERE page_title LIKE ? ESCAPE '!' {namespace_condition}
         ORDER BY page_namespace, page_title LIMIT {MAX_SUBPAGES}"
    );
    let rows: Vec<(i64, String)> = match namespace {
        -1 => conn.exec(sql, (pattern,)).await,
        ns => conn.exec(sql, (pattern, ns)).await,
    }
    .map_err(anyhow::Error::from)?;
    let parent = if namespace == -1 {
        parsed.pretty().to_string()
    } else if namespace == parsed.namespace_id() {
        parsed.full_pretty(&api).unwrap_or_else(|| parsed.pretty().to_string())
    } else {
        let title = mediawiki::Title::new(parsed.pretty(), namespace);
        title.full_pretty(&api).unwrap_or_else(|| title.pretty().to_string())
    };
    let count = rows.len();
    let truncated = count as u64 >= MAX_SUBPAGES;
    let mut subpages: Vec<String> = rows
        .into_iter()
        .map(|(namespace, title)| {
            let title = mediawiki::Title::new(&title, namespace);
            title.full_pretty(&api).unwrap_or_else(|| title.pretty().to_string())
        })
        .collect();
    subpages.insert(0, parent.clone());
    Ok(Results { parent, count, subpages: subpages.join("\n"), truncated })
}

/// `value` for a `LIKE` pattern, with its wildcards taken literally.
fn like_escape(value: &str) -> String {
    value.replace('!', "!!").replace('%', "!%").replace('_', "!_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn like_patterns() {
        assert_eq!(like_escape("A_B"), "A!_B");
        assert_eq!(like_escape("100%"), "100!%");
        assert_eq!(like_escape("Hi!"), "Hi!!");
    }
}
