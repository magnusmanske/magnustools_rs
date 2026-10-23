//! `uploader_distribution.php`: for the files in a category tree
//! (optionally intersected with more category trees), how many of the
//! tree's categories each uploader contributed to.

use crate::replicas::{CATEGORY_TABLES, display_title, select_in};
use crate::state::AppState;
use crate::web::{AppError, Params, render};
use crate::wiki::Wiki;
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

const NS_FILE: i64 = 6;
const DEPTH: u32 = 12;
const MAX_FILES: usize = 30_000;

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default)]
    cat1: String,
    #[serde(default)]
    cat2: String,
    #[serde(default = "default_language")]
    lang: String,
    #[serde(default = "default_project")]
    project: String,
    doit: Option<String>,
}

fn default_language() -> String {
    "commons".to_string()
}

fn default_project() -> String {
    "wikimedia".to_string()
}

struct Uploader {
    name: String,
    url: String,
    categories: Vec<String>,
}

struct Results {
    files: usize,
    capped: bool,
    uploaders: Vec<Uploader>,
}

#[derive(Template)]
#[template(path = "tools/uploader_distribution.html")]
struct Page<'a> {
    title: &'static str,
    request: &'a Request,
    results: Option<Results>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let results = match request.doit.is_some() && !request.cat1.trim().is_empty() {
        true => Some(results(&state, &request).await?),
        false => None,
    };
    render(&Page { title: "Uploader distribution", request: &request, results })
}

async fn results(state: &AppState, request: &Request) -> Result<Results, AppError> {
    let wiki = Wiki::new(&request.lang, &request.project)?;
    let tree = state.replicas.category_tree(&wiki, request.cat1.trim(), DEPTH).await?;
    let mut conn = state.replicas.conn(&wiki, CATEGORY_TABLES).await?;
    let sql = "SELECT page_title, lt_title FROM page
        JOIN categorylinks ON cl_from=page_id JOIN linktarget ON cl_target_id=lt_id
        WHERE page_namespace=6 AND lt_namespace=14 AND lt_title IN";
    let pairs: Vec<(String, String)> = select_in(&mut conn, sql, "", &[], &tree).await?;
    // A file in several categories of the tree counts for the first.
    let mut category_of: HashMap<String, String> = HashMap::new();
    for (file, category) in pairs {
        category_of.entry(file).or_insert(category);
    }
    for other in request.cat2.lines().map(str::trim).filter(|c| !c.is_empty()) {
        let files: HashSet<String> =
            state.replicas.pages_in_tree(&wiki, other, DEPTH, NS_FILE).await?.into_iter().collect();
        category_of.retain(|file, _| files.contains(file));
    }
    let mut files: Vec<String> = category_of.keys().cloned().collect();
    files.sort();
    let capped = files.len() > MAX_FILES;
    files.truncate(MAX_FILES);

    let mut core = state.replicas.conn(&wiki, &["file", "filerevision", "actor"]).await?;
    let sql = "SELECT file_name, actor_name FROM file
        JOIN filerevision ON fr_id=file_latest JOIN actor ON actor_id=fr_actor
        WHERE file_deleted=0 AND file_name IN";
    let uploads: Vec<(String, String)> = select_in(&mut core, sql, "", &[], &files).await?;
    let uploaders = by_uploader(&uploads, &category_of)
        .into_iter()
        .map(|(name, categories)| Uploader {
            url: wiki.page_url(&format!("User:{name}")),
            name,
            categories: categories.iter().map(|c| display_title(c)).collect(),
        })
        .collect();
    Ok(Results { files: files.len(), capped, uploaders })
}

/// Uploader and the categories of their files, most categories first.
fn by_uploader(uploads: &[(String, String)], category_of: &HashMap<String, String>) -> Vec<(String, BTreeSet<String>)> {
    let mut categories: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (file, uploader) in uploads {
        if let Some(category) = category_of.get(file) {
            categories.entry(uploader.clone()).or_default().insert(category.clone());
        }
    }
    let mut uploaders: Vec<_> = categories.into_iter().collect();
    uploaders.sort_by_key(|(_, cats)| std::cmp::Reverse(cats.len()));
    uploaders
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grouped_by_uploader() {
        let pairs = |p: &[(&str, &str)]| p.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect::<Vec<_>>();
        let uploads =
            pairs(&[("a.jpg", "Ann"), ("b.jpg", "Bob"), ("c.jpg", "Bob"), ("d.jpg", "Bob"), ("x.jpg", "Eve")]);
        let category_of: HashMap<String, String> =
            pairs(&[("a.jpg", "Cats"), ("b.jpg", "Cats"), ("c.jpg", "Dogs"), ("d.jpg", "Dogs")]).into_iter().collect();
        let result = by_uploader(&uploads, &category_of);
        let summary: Vec<(&str, usize)> = result.iter().map(|(u, c)| (u.as_str(), c.len())).collect();
        assert_eq!(summary, vec![("Bob", 2), ("Ann", 1)]);
    }
}
