//! `subpages.php`: the subpages of a page, for a raw watchlist.

use crate::mediawiki;
use crate::replicas::{db_title, display_title};
use crate::state::AppState;
use crate::web::{AppError, Params, render};
use crate::wiki::Wiki;
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use serde::Deserialize;
use std::collections::HashMap;
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

async fn results(state: &AppState, request: &Request, namespace: i64) -> Result<Results, AppError> {
    let wiki = Wiki::new(&request.lang, &request.family)?;
    let names = mediawiki::namespace_names(&state.http, &wiki).await.unwrap_or_default();
    let (namespace, title) = split_namespace(namespace, &request.title, &names);
    let mut conn = state.replicas.conn(&wiki, &["page"]).await?;
    let title = db_title(&title);
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
    let parent = mediawiki::title_with_namespace(namespace, &display_title(&title), &names);
    let count = rows.len();
    let truncated = count as u64 >= MAX_SUBPAGES;
    let mut subpages: Vec<String> = rows
        .into_iter()
        .map(|(namespace, title)| mediawiki::title_with_namespace(namespace, &display_title(&title), &names))
        .collect();
    subpages.insert(0, parent.clone());
    Ok(Results { parent, count, subpages: subpages.join("\n"), truncated })
}

/// A namespace given as a prefix on the title, if the wiki knows it.
fn split_namespace(namespace: i64, title: &str, names: &HashMap<i64, String>) -> (i64, String) {
    let Some((prefix, rest)) = title.split_once(':') else {
        return (namespace, title.to_string());
    };
    match names.iter().find(|(id, name)| **id > 0 && name.eq_ignore_ascii_case(prefix)) {
        Some((id, _)) => (*id, rest.to_string()),
        None => (namespace, title.to_string()),
    }
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

    #[test]
    fn titles_can_carry_their_namespace() {
        let names = HashMap::from([(0, "Article".to_string()), (2, "User".to_string())]);
        assert_eq!(split_namespace(2, "Magnus_Manske", &names), (2, "Magnus_Manske".to_string()));
        assert_eq!(split_namespace(0, "User:Magnus_Manske", &names), (2, "Magnus_Manske".to_string()));
        assert_eq!(split_namespace(0, "Star_Trek:_Voyager", &names), (0, "Star_Trek:_Voyager".to_string()));
    }
}
