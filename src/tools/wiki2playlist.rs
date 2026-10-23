//! `wiki2playlist.php`: an M3U playlist of the media files listed on a wiki
//! page (`* File.ogg` lines) or in a category tree.

use crate::mediawiki::{self, strip_namespace};
use crate::state::AppState;
use crate::web::{AppError, Params, render};
use crate::wiki::{Wiki, encode_title};
use askama::Template;
use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

const MAX_DEPTH: u32 = 5;
const MAX_FILES: usize = 2000;
/// Titles per `imageinfo` request.
const BATCH: usize = 50;
const FILE_NAMESPACES: &[&str] = &["file", "image", "datei", "bild"];

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default = "default_language")]
    language: String,
    #[serde(default = "default_project")]
    project: String,
    #[serde(default)]
    page: String,
    #[serde(default)]
    category: String,
    #[serde(default)]
    depth: String,
    doit: Option<String>,
}

fn default_language() -> String {
    "commons".to_string()
}

fn default_project() -> String {
    "wikimedia".to_string()
}

#[derive(Template)]
#[template(path = "tools/wiki2playlist.html")]
struct Page<'a> {
    title: &'static str,
    request: &'a Request,
    error: Option<String>,
}

#[derive(Debug, PartialEq)]
struct Entry {
    title: String,
    url: String,
    seconds: Option<u64>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Response, AppError> {
    let page = request.page.trim();
    let category = request.category.trim();
    if request.doit.is_none() || (page.is_empty() && category.is_empty()) {
        let error = request.doit.is_some().then(|| "Enter a page or a category.".to_string());
        return Ok(render(&Page { title: "Wiki to playlist", request: &request, error })?.into_response());
    }
    let wiki = Wiki::new(&request.language, &request.project)?;
    let (files, name) = if !category.is_empty() {
        let depth = request.depth.trim().parse().unwrap_or(0).min(MAX_DEPTH);
        let files = mediawiki::category_files(&state.http, &wiki, category, depth, MAX_FILES).await?;
        (files, category.replace(' ', "_"))
    } else {
        let text = mediawiki::page_wikitext(&state.http, &wiki, page).await?.unwrap_or_default();
        (listed_files(&text), "playlist".to_string())
    };
    let info = media_info(&state.http, &wiki, &files).await?;
    let entries: Vec<Entry> = files
        .into_iter()
        .filter_map(|file| {
            let info = info.get(&file);
            // Pages list what they want; of a category's files, only audio and video.
            if !category.is_empty() && !info.is_some_and(|i| i.is_media) {
                return None;
            }
            Some(Entry {
                title: display_title(&file),
                url: format!("https://{}/wiki/Special:Redirect/file/{}", wiki.host(), encode_title(&file)),
                seconds: info.and_then(|i| i.seconds),
            })
        })
        .collect();
    let filename =
        format!("{}-{}.m3u", wiki.host(), name).replace(|c: char| !c.is_alphanumeric() && !"._-".contains(c), "_");
    Ok((
        [
            (header::CONTENT_TYPE, "audio/mpegurl".to_string()),
            (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{filename}\"")),
        ],
        m3u(&entries),
    )
        .into_response())
}

/// File names from the `*` list items of a page.
fn listed_files(wikitext: &str) -> Vec<String> {
    wikitext
        .lines()
        .filter_map(|line| line.strip_prefix('*'))
        .map(|item| item.trim_start_matches('*').trim())
        .map(|item| strip_namespace(item, FILE_NAMESPACES).unwrap_or(item).replace('_', " "))
        .filter(|item| !item.is_empty())
        .collect()
}

/// The file name without extension, as a track title.
fn display_title(file: &str) -> String {
    let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem).replace('_', " ");
    let stem = stem.trim();
    let mut chars = stem.chars();
    chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
}

fn m3u(entries: &[Entry]) -> String {
    let mut out = String::from("#EXTM3U\n");
    for e in entries {
        let seconds = e.seconds.map_or_else(|| "-1".to_string(), |s| s.to_string());
        out.push_str(&format!("#EXTINF:{seconds},{}\n{}\n", e.title.replace('\n', " "), e.url));
    }
    out
}

struct MediaInfo {
    is_media: bool,
    seconds: Option<u64>,
}

/// Media type and play length of files, by file name.
async fn media_info(
    http: &reqwest::Client,
    wiki: &Wiki,
    files: &[String],
) -> anyhow::Result<HashMap<String, MediaInfo>> {
    let mut info = HashMap::new();
    for batch in files.chunks(BATCH) {
        let titles = batch.iter().map(|f| format!("File:{f}")).collect::<Vec<_>>().join("|");
        let params =
            [("action", "query"), ("prop", "imageinfo"), ("iiprop", "mediatype|metadata"), ("titles", titles.as_str())];
        let json = mediawiki::api(http, wiki, &params).await?;
        for page in json["query"]["pages"].as_array().into_iter().flatten() {
            let Some(title) = page["title"].as_str() else { continue };
            let name = title.split_once(':').map_or(title, |(_, n)| n).to_string();
            let ii = &page["imageinfo"][0];
            let is_media = matches!(ii["mediatype"].as_str(), Some("AUDIO" | "VIDEO"));
            info.insert(name, MediaInfo { is_media, seconds: length(&ii["metadata"]) });
        }
    }
    Ok(info)
}

fn length(metadata: &Value) -> Option<u64> {
    let entry = metadata.as_array()?.iter().find(|m| m["name"] == "length")?;
    // Rounded; lengths are small positive numbers of seconds.
    entry["value"].as_f64().filter(|v| v.is_finite() && *v >= 0.0).map(|v| v.round() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn files_from_a_page() {
        let text = "Intro\n* Example.ogg\n** File:Bach_fugue.oga\n*\nNot this.ogg";
        assert_eq!(listed_files(text), vec!["Example.ogg", "Bach fugue.oga"]);
    }

    #[test]
    fn titles_and_playlist() {
        assert_eq!(display_title("bach_fugue.oga"), "Bach fugue");
        let entries = vec![
            Entry { title: "A".into(), url: "https://x/A.ogg".into(), seconds: Some(6) },
            Entry { title: "B".into(), url: "https://x/B.ogg".into(), seconds: None },
        ];
        assert_eq!(m3u(&entries), "#EXTM3U\n#EXTINF:6,A\nhttps://x/A.ogg\n#EXTINF:-1,B\nhttps://x/B.ogg\n");
    }

    #[test]
    fn lengths() {
        assert_eq!(length(&json!([{"name": "version", "value": 2}, {"name": "length", "value": 6.1}])), Some(6));
        assert_eq!(length(&json!([])), None);
        assert_eq!(length(&Value::Null), None);
    }
}
