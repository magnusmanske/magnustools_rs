//! `bong.php`: "BONG", a pun on Bing. One search box over several Wikimedia
//! projects: encyclopedia, images, audio and video, news, dictionary and maps.
//! (Maps came from GeoNames; now they are Wikipedia articles with coordinates.)

use crate::mediawiki::{self, html_to_text};
use crate::state::AppState;
use crate::web::{AppError, Params, render};
use crate::wiki::{Wiki, encode_component};
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

const TEXT_RESULTS: &str = "20";
const MEDIA_RESULTS: &str = "48";
const THUMB_WIDTH: &str = "180";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Encyclopedia,
    Images,
    AudioVideo,
    News,
    Dictionary,
    Maps,
}

/// Mode, its name in URLs, its label.
const MODES: &[(Mode, &str, &str)] = &[
    (Mode::Encyclopedia, "pedia", "Encyclopedia"),
    (Mode::Images, "image", "Images"),
    (Mode::AudioVideo, "av", "Audio/Video"),
    (Mode::News, "news", "News"),
    (Mode::Dictionary, "dict", "Dictionary"),
    (Mode::Maps, "map", "Maps"),
];

impl Mode {
    /// Unknown names give the encyclopedia.
    fn parse(name: &str) -> Self {
        MODES.iter().find(|(_, n, _)| *n == name.trim()).map_or(Self::Encyclopedia, |(mode, ..)| *mode)
    }

    fn name(self) -> &'static str {
        MODES.iter().find(|(m, ..)| *m == self).map_or("pedia", |(_, name, _)| name)
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Request {
    q: String,
    mode: String,
}

struct Hit {
    title: String,
    url: String,
    text: String,
    thumbnail: Option<String>,
}

struct Tab {
    label: &'static str,
    url: String,
    current: bool,
}

#[derive(Template)]
#[template(path = "tools/bong.html")]
struct Page<'a> {
    title: &'static str,
    q: &'a str,
    mode: &'static str,
    tabs: Vec<Tab>,
    /// Images are shown as a grid; everything else as a list.
    grid: bool,
    hits: Option<Vec<Hit>>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Html<String>, AppError> {
    let mode = Mode::parse(&request.mode);
    let q = request.q.trim();
    let tabs = MODES
        .iter()
        .map(|(m, name, label)| Tab {
            label,
            url: format!("/bong/?mode={name}&q={}", encode_component(q)),
            current: *m == mode,
        })
        .collect();
    let hits = match q.is_empty() {
        true => None,
        false => Some(search(&state.http, mode, q).await?),
    };
    render(&Page { title: "BONG", q, mode: mode.name(), tabs, grid: mode == Mode::Images, hits })
}

async fn search(http: &reqwest::Client, mode: Mode, q: &str) -> anyhow::Result<Vec<Hit>> {
    let wiki = |language, project| Wiki::new(language, project).map_err(anyhow::Error::from);
    match mode {
        Mode::Encyclopedia => text_search(http, &wiki("en", "wikipedia")?, q).await,
        Mode::News => text_search(http, &wiki("en", "wikinews")?, q).await,
        Mode::Dictionary => text_search(http, &wiki("en", "wiktionary")?, q).await,
        Mode::Images => media_search(http, &format!("{q} filetype:bitmap|drawing")).await,
        Mode::AudioVideo => media_search(http, &format!("{q} filetype:audio|video")).await,
        Mode::Maps => place_search(http, &wiki("en", "wikipedia")?, q).await,
    }
}

/// Pages with a snippet of where they match.
async fn text_search(http: &reqwest::Client, wiki: &Wiki, q: &str) -> anyhow::Result<Vec<Hit>> {
    let params = [("action", "query"), ("list", "search"), ("srsearch", q), ("srlimit", TEXT_RESULTS)];
    let json = mediawiki::api(http, wiki, &params).await?;
    Ok(json["query"]["search"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|hit| {
            let title = hit["title"].as_str()?;
            Some(Hit {
                title: title.to_string(),
                url: wiki.page_url(title),
                text: html_to_text(hit["snippet"].as_str().unwrap_or_default()),
                thumbnail: None,
            })
        })
        .collect())
}

/// Commons files, with thumbnails.
async fn media_search(http: &reqwest::Client, q: &str) -> anyhow::Result<Vec<Hit>> {
    let commons = Wiki::commons();
    let params = [
        ("action", "query"),
        ("generator", "search"),
        ("gsrsearch", q),
        ("gsrnamespace", "6"),
        ("gsrlimit", MEDIA_RESULTS),
        ("prop", "imageinfo"),
        ("iiprop", "url|mediatype"),
        ("iiurlwidth", THUMB_WIDTH),
    ];
    let json = mediawiki::api(http, &commons, &params).await?;
    Ok(search_results(&json)
        .into_iter()
        .filter_map(|page| {
            let title = page["title"].as_str()?;
            let info = &page["imageinfo"][0];
            Some(Hit {
                title: title.trim_start_matches("File:").replace('_', " "),
                url: commons.page_url(title),
                text: info["mediatype"].as_str().unwrap_or_default().to_lowercase(),
                thumbnail: info["thumburl"].as_str().map(str::to_string),
            })
        })
        .collect())
}

/// Articles with coordinates, linked to OpenStreetMap.
async fn place_search(http: &reqwest::Client, wiki: &Wiki, q: &str) -> anyhow::Result<Vec<Hit>> {
    let params = [
        ("action", "query"),
        ("generator", "search"),
        ("gsrsearch", q),
        ("gsrlimit", TEXT_RESULTS),
        ("prop", "coordinates|description"),
        ("colimit", "max"),
    ];
    let json = mediawiki::api(http, wiki, &params).await?;
    Ok(search_results(&json)
        .into_iter()
        .filter_map(|page| {
            let title = page["title"].as_str()?;
            let coordinates = &page["coordinates"][0];
            let (lat, lon) = (coordinates["lat"].as_f64()?, coordinates["lon"].as_f64()?);
            Some(Hit {
                title: title.to_string(),
                url: format!("https://www.openstreetmap.org/?mlat={lat}&mlon={lon}#map=12/{lat}/{lon}"),
                text: page["description"].as_str().unwrap_or_default().to_string(),
                thumbnail: None,
            })
        })
        .collect())
}

/// The pages of a `generator=search` query, in search order.
fn search_results(json: &Value) -> Vec<Value> {
    let mut pages: Vec<Value> = json["query"]["pages"].as_array().cloned().unwrap_or_default();
    pages.sort_by_key(|p| p["index"].as_u64().unwrap_or(u64::MAX));
    pages
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn modes() {
        assert_eq!(Mode::parse("image"), Mode::Images);
        assert_eq!(Mode::parse("nonsense"), Mode::Encyclopedia);
        assert_eq!(Mode::Maps.name(), "map");
    }

    #[test]
    fn generator_results_in_search_order() {
        let json = json!({"query": {"pages": [{"title": "B", "index": 2}, {"title": "A", "index": 1}]}});
        let titles: Vec<_> = search_results(&json).iter().map(|p| p["title"].clone()).collect();
        assert_eq!(titles, ["A", "B"]);
    }
}
