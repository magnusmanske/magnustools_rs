//! `filterdone.php`: removes the entries from a wiki page's list whose
//! linked page already exists.

use crate::mediawiki::{Page as WikiPage, Title};
use crate::replicas::{db_title, select_in};
use crate::state::AppState;
use crate::web::{AppError, Params, render};
use crate::wiki::Wiki;
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use wikimisc::mysql_async::Value;

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default = "default_language")]
    language: String,
    #[serde(default = "default_project")]
    project: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    doit: Option<String>,
}

fn default_language() -> String {
    "en".to_string()
}

fn default_project() -> String {
    "wikipedia".to_string()
}

struct Results {
    wiki: String,
    title: String,
    removed: usize,
    kept: usize,
    output: String,
}

#[derive(Template)]
#[template(path = "tools/filterdone.html")]
struct Page<'a> {
    title: &'static str,
    request: &'a Request,
    results: Option<Results>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let results = match request.doit.is_some() && !request.title.trim().is_empty() {
        true => Some(results(&state, &request).await?),
        false => None,
    };
    render(&Page { title: "Filter done", request: &request, results })
}

async fn results(state: &AppState, request: &Request) -> Result<Results, AppError> {
    let wiki = Wiki::new(&request.language, &request.project)?;
    let api = state.api(&wiki).await?;
    let title = Title::new_from_full(request.title.trim(), &api);
    let mut page = WikiPage::new(title.clone());
    let text = page.text(&api).await?;

    // The first link of each list line, normalized to a database title.
    let mut targets: HashSet<(i64, String)> = HashSet::new();
    for line in text.lines().filter(|line| is_list_line(line)) {
        if let Some(target) = first_link_target(line) {
            let target = Title::new_from_full(target, &api);
            let namespace = target.namespace_id();
            targets.insert((namespace, db_title(&target.with_underscores())));
        }
    }
    let mut by_namespace: HashMap<i64, Vec<String>> = HashMap::new();
    for (namespace, title) in targets {
        by_namespace.entry(namespace).or_default().push(title);
    }
    let mut conn = state.replicas.conn(&wiki, &["page"]).await?;
    let mut existing: HashSet<(i64, String)> = HashSet::new();
    for (namespace, titles) in &by_namespace {
        let sql = "SELECT page_title FROM page WHERE page_namespace=? AND page_title IN";
        let rows: Vec<String> = select_in(&mut conn, sql, "", &[Value::from(*namespace)], titles).await?;
        existing.extend(rows.into_iter().map(|title| (*namespace, title)));
    }

    let mut kept = vec![];
    let mut removed = 0;
    for line in text.lines() {
        let target = if is_list_line(line) { first_link_target(line) } else { None };
        let exists = target.is_some_and(|target| {
            let target = Title::new_from_full(target, &api);
            existing.contains(&(target.namespace_id(), db_title(&target.with_underscores())))
        });
        match exists {
            true => removed += 1,
            false => kept.push(line),
        }
    }
    let mut output = kept.join("\n");
    if !output.is_empty() {
        output.push('\n');
    }
    Ok(Results {
        wiki: wiki.to_string(),
        title: title.full_pretty(&api).unwrap_or_else(|| title.pretty().to_string()),
        removed,
        kept: kept.len(),
        output,
    })
}

fn is_list_line(line: &str) -> bool {
    line.starts_with('*') || line.starts_with('#')
}

/// The target of the first wikilink on a line, without anchor or label.
fn first_link_target(line: &str) -> Option<&str> {
    let start = line.find("[[")? + 2;
    let rest = &line[start..];
    let end = rest.find(['|', '#', ']']).unwrap_or(rest.len());
    let target = rest[..end].trim();
    (!target.is_empty()).then_some(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_wikilink() {
        assert_eq!(first_link_target("* [[Foo|shown]]"), Some("Foo"));
        assert_eq!(first_link_target("# [[Foo#Section]]"), Some("Foo"));
        assert_eq!(first_link_target("* [[Foo]] and [[Bar]]"), Some("Foo"));
        assert_eq!(first_link_target("* plain text"), None);
        assert_eq!(first_link_target("* [[|label]]"), None);
    }

    #[test]
    fn only_list_lines_are_filtered() {
        assert!(is_list_line("* entry"));
        assert!(is_list_line("# entry"));
        assert!(!is_list_line("entry"));
        assert!(!is_list_line("== Section =="));
    }
}
