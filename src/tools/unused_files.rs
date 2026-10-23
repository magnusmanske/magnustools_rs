//! `unused_files.php`: files in a Commons category tree that no wiki uses
//! in an article (or none of the given wikis).

use crate::replicas::{display_title, select_in};
use crate::state::AppState;
use crate::web::{AppError, Params, render};
use crate::wiki::{Wiki, encode_component, encode_title};
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use serde::Deserialize;
use std::collections::HashSet;
use std::sync::Arc;
use wikimisc::mysql_async::Value;

const NS_FILE: i64 = 6;
const DEFAULT_DEPTH: u32 = 12;
const MAX_DEPTH: u32 = 20;
const THUMB_SIZE: u32 = 200;
/// Shown on the page; the count covers all.
const MAX_SHOWN: usize = 2000;

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default)]
    category: String,
    depth: Option<String>,
    #[serde(default)]
    projects: String,
}

struct File {
    name: String,
    url: String,
    thumbnail: Option<String>,
    thumb_padding: u32,
    search_url: String,
    search_any_url: String,
}

struct Results {
    total: usize,
    unused: usize,
    files: Vec<File>,
}

#[derive(Template)]
#[template(path = "tools/unused_files.html")]
struct Page<'a> {
    title: &'static str,
    category: &'a str,
    depth: u32,
    projects: &'a str,
    results: Option<Results>,
    thumb_size: u32,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let depth = request.depth.as_deref().and_then(|d| d.trim().parse().ok()).unwrap_or(DEFAULT_DEPTH).min(MAX_DEPTH);
    let category = request.category.trim();
    let projects = request.projects.trim();
    let results = match category.is_empty() {
        true => None,
        false => Some(results(&state, category, depth, projects).await?),
    };
    render(&Page { title: "Unused files", category, depth, projects, results, thumb_size: THUMB_SIZE })
}

async fn results(state: &AppState, category: &str, depth: u32, projects: &str) -> Result<Results, AppError> {
    let wikis: Vec<String> = projects.split(',').map(str::trim).filter(|p| !p.is_empty()).map(str::to_string).collect();
    if let Some(bad) = wikis.iter().find(|w| !w.chars().all(|c| c.is_ascii_lowercase() || c == '_')) {
        return Err(AppError::BadRequest(format!("“{bad}” is not a wiki database name like enwiki or dewikisource.")));
    }
    let commons = Wiki::commons();
    let files = state.replicas.pages_in_tree(&commons, category, depth, NS_FILE).await?;
    let total = files.len();

    // The link tables are on another server than the file tables, so this cannot be a join.
    let mut links = state.replicas.conn(&commons, &["globalimagelinks"]).await?;
    let wiki_filter = match wikis.is_empty() {
        true => String::new(),
        false => format!("AND gil_wiki IN ({})", vec!["?"; wikis.len()].join(",")),
    };
    let sql = format!(
        "SELECT DISTINCT gil_to FROM globalimagelinks WHERE gil_page_namespace_id=0 {wiki_filter} AND gil_to IN"
    );
    let params: Vec<Value> = wikis.iter().map(|w| Value::from(w.as_str())).collect();
    let used: HashSet<String> = select_in(&mut links, &sql, "", &params, &files).await?.into_iter().collect();
    let unused: Vec<String> = files.iter().filter(|f| !used.contains(*f)).cloned().collect();

    let mut core = state.replicas.conn(&commons, &["file", "filerevision"]).await?;
    let sql = "SELECT file_name, fr_width, fr_height FROM file JOIN filerevision ON fr_id=file_latest
        WHERE file_deleted=0 AND file_name IN";
    let mut rows: Vec<(String, u32, u32)> = select_in(&mut core, sql, "", &[], &unused).await?;
    rows.sort();
    let unused = rows.len();
    let files = rows.into_iter().take(MAX_SHOWN).map(|(name, w, h)| file(&commons, &name, w, h)).collect();
    Ok(Results { total, unused, files })
}

fn file(commons: &Wiki, name: &str, width: u32, height: u32) -> File {
    // Scaled to fit a square; files without a size (audio) get no thumbnail.
    let thumb_width = match (width, height) {
        (0, _) | (_, 0) => 0,
        (w, h) if w < h => (THUMB_SIZE * w / h).max(1),
        _ => THUMB_SIZE,
    };
    let thumbnail = (thumb_width > 0).then(|| {
        format!("https://{}/wiki/Special:Redirect/file/{}?width={thumb_width}", commons.host(), encode_title(name))
    });
    let stem = display_title(name.rsplit_once('.').map_or(name, |(stem, _)| stem));
    let search = |terms: &str| {
        format!(
            "https://www.google.com/search?q={}",
            encode_component(&format!("{terms} site:wikipedia.org -site:commons.wikimedia.org"))
        )
    };
    File {
        name: display_title(name),
        url: commons.page_url(&format!("File:{name}")),
        thumbnail,
        thumb_padding: (THUMB_SIZE - thumb_width.min(THUMB_SIZE)) / 2,
        search_url: search(&stem),
        search_any_url: search(&stem.split(' ').collect::<Vec<_>>().join(" OR ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumbnails_and_searches() {
        let portrait = file(&Wiki::commons(), "Bf_109_(2).jpg", 1000, 2000);
        assert_eq!(portrait.name, "Bf 109 (2).jpg");
        assert_eq!(
            portrait.thumbnail.as_deref(),
            Some("https://commons.wikimedia.org/wiki/Special:Redirect/file/Bf_109_(2).jpg?width=100")
        );
        assert_eq!(portrait.thumb_padding, 50);
        assert!(portrait.search_any_url.contains("Bf%20OR%20109%20OR%20%282%29"));
        assert_eq!(file(&Wiki::commons(), "Sound.ogg", 0, 0).thumbnail, None);
    }
}
