//! `flickrfree.php`: the latest photos on Flickr under a licence that
//! allows uploading them to Commons.

use crate::state::AppState;
use crate::web::{AppError, render};
use anyhow::Context;
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use chrono::DateTime;
use serde::Deserialize;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

const FLICKR_API: &str = "https://api.flickr.com/services/rest/";
/// Flickr is asked at most this often; everyone in between sees the same list.
const CACHE_FOR: Duration = Duration::from_secs(600);

/// Flickr licence IDs that Commons accepts.
const LICENSES: &[(&str, &str)] = &[
    ("4", "CC BY 2.0"),
    ("5", "CC BY-SA 2.0"),
    ("7", "No known copyright restrictions"),
    ("8", "U.S. Government work"),
    ("9", "CC0"),
    ("10", "Public Domain Mark"),
];

#[derive(Debug, Clone, Deserialize)]
struct Photo {
    id: String,
    owner: String,
    #[serde(default)]
    title: String,
    license: String,
    #[serde(default)]
    dateupload: String,
    #[serde(default)]
    ownername: String,
    #[serde(default)]
    url_t: String,
    #[serde(default)]
    tags: String,
}

impl Photo {
    fn page_url(&self) -> String {
        format!("https://www.flickr.com/photos/{}/{}", self.owner, self.id)
    }

    fn owner_url(&self) -> String {
        format!("https://www.flickr.com/photos/{}", self.owner)
    }

    fn upload_url(&self) -> String {
        format!("https://flickr2commons.toolforge.org/?photoid={}", self.id)
    }

    fn license_name(&self) -> &'static str {
        LICENSES.iter().find(|(id, _)| *id == self.license).map_or("", |(_, name)| name)
    }

    fn uploaded(&self) -> String {
        self.dateupload
            .parse()
            .ok()
            .and_then(|seconds| DateTime::from_timestamp(seconds, 0))
            .map_or_else(String::new, |t| t.format("%Y-%m-%d %H:%M UTC").to_string())
    }
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    photos: Photos,
}

#[derive(Debug, Deserialize)]
struct Photos {
    photo: Vec<Photo>,
}

#[derive(Template)]
#[template(path = "tools/flickrfree.html")]
struct Page {
    title: &'static str,
    photos: Arc<Vec<Photo>>,
}

/// The last list, and when it was fetched.
type Cached = Option<(Instant, Arc<Vec<Photo>>)>;

static CACHE: LazyLock<Mutex<Cached>> = LazyLock::new(|| Mutex::new(None));

pub async fn handler(State(state): State<Arc<AppState>>) -> Result<Html<String>, AppError> {
    let mut cache = CACHE.lock().await;
    let photos = match cache.as_ref() {
        Some((fetched, photos)) if fetched.elapsed() < CACHE_FOR => photos.clone(),
        _ => {
            let photos = Arc::new(latest_photos(&state).await?);
            *cache = Some((Instant::now(), photos.clone()));
            photos
        }
    };
    drop(cache);
    render(&Page { title: "FlickrFree", photos })
}

async fn latest_photos(state: &AppState) -> Result<Vec<Photo>, AppError> {
    let key_file = state.config.flickr_key_file.as_ref().ok_or_else(|| AppError::Internal("no Flickr key".into()))?;
    let key = tokio::fs::read_to_string(key_file).await.context("cannot read the Flickr key")?;
    let licenses = LICENSES.iter().map(|(id, _)| *id).collect::<Vec<_>>().join(",");
    let response: SearchResponse = state
        .http
        .get(FLICKR_API)
        .query(&[
            ("method", "flickr.photos.search"),
            ("api_key", key.trim()),
            ("license", &licenses),
            ("per_page", "500"),
            ("sort", "date-posted-desc"),
            ("extras", "license,date_upload,owner_name,url_t,tags"),
            ("format", "json"),
            ("nojsoncallback", "1"),
        ])
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .context("Flickr search failed")?
        .json()
        .await
        .context("unexpected answer from Flickr")?;
    let mut seen = std::collections::HashSet::new();
    Ok(response
        .photos
        .photo
        .into_iter()
        .filter(|p| !p.license_name().is_empty() && seen.insert(p.id.clone()))
        .collect())
}
