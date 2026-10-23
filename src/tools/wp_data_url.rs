//! `wp_data_url.php`: a Wikipedia article as one self-contained HTML page,
//! its images inlined as `data:` URLs (e.g. for offline reading).

use crate::state::AppState;
use crate::web::{AppError, Params, render};
use crate::wiki::Wiki;
use anyhow::Context;
use askama::Template;
use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use regex::Regex;
use serde::Deserialize;
use std::sync::{Arc, LazyLock};
use tokio::task::JoinSet;

/// Each request makes us fetch every image of an article, so cap that.
const MAX_IMAGES: usize = 100;
const MAX_IMAGE_BYTES: usize = 5 << 20;
const MAX_TOTAL_BYTES: usize = 30 << 20;
const PARALLEL_FETCHES: usize = 6;

#[derive(Debug, Default, Deserialize)]
pub struct Request {
    #[serde(default)]
    lang: String,
    #[serde(default)]
    title: String,
}

#[derive(Template)]
#[template(path = "tools/wp_data_url.html")]
struct Page {
    title: &'static str,
}

static IMAGE_SRC: LazyLock<Regex> = LazyLock::new(|| {
    // A constant pattern, covered by the tests.
    #[allow(clippy::expect_used)]
    Regex::new(r#"<img [^>]*?src="(//(?:upload|thumb)\.wikimedia\.org/[^"]+)""#).expect("valid regex")
});

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Response, AppError> {
    let (lang, title) = (request.lang.trim(), request.title.trim());
    if lang.is_empty() || title.is_empty() {
        return Ok(render(&Page { title: "Wikipedia article with inline images" })?.into_response());
    }
    let wiki = Wiki::new(lang, "wikipedia")?;
    let response = state.http.get(wiki.page_url(title)).send().await.context("cannot fetch the article")?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(AppError::NotFound);
    }
    let mut html = response
        .error_for_status()
        .context("cannot fetch the article")?
        .text()
        .await
        .context("cannot read the article")?;

    for (url, data_url) in inline_images(&state.http, image_urls(&html)).await {
        html = html.replace(&url, &data_url);
    }
    // Stylesheets and links are relative to Wikipedia.
    html = html.replacen("<head>", &format!("<head><base href=\"https://{}/\">", wiki.host()), 1);
    Ok((
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            // Wikipedia's page, not ours: no scripts, and no access to this site.
            (header::CONTENT_SECURITY_POLICY, "sandbox"),
        ],
        html,
    )
        .into_response())
}

/// Distinct image URLs as they appear in the HTML (protocol-relative, `&` as `&amp;`), in page order.
fn image_urls(html: &str) -> Vec<String> {
    let mut urls: Vec<String> = vec![];
    for captures in IMAGE_SRC.captures_iter(html) {
        if let Some(url) = captures.get(1).map(|m| m.as_str().to_string())
            && !urls.contains(&url)
        {
            urls.push(url);
        }
    }
    urls.truncate(MAX_IMAGES);
    urls
}

/// `(url, data URL)` for the images that could be fetched within the limits.
async fn inline_images(http: &reqwest::Client, urls: Vec<String>) -> Vec<(String, String)> {
    let mut inlined = vec![];
    let mut total = 0;
    for chunk in urls.chunks(PARALLEL_FETCHES) {
        let mut fetches = JoinSet::new();
        for url in chunk {
            let (http, url) = (http.clone(), url.clone());
            fetches.spawn(async move {
                let image = fetch_image(&http, &url).await;
                (url, image)
            });
        }
        while let Some(Ok((url, image))) = fetches.join_next().await {
            let Some((mime, bytes)) = image else { continue };
            total += bytes.len();
            if total > MAX_TOTAL_BYTES {
                return inlined;
            }
            let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
            inlined.push((url, format!("data:{mime};base64,{data}")));
        }
    }
    inlined
}

async fn fetch_image(http: &reqwest::Client, url: &str) -> Option<(String, Vec<u8>)> {
    let url = format!("https:{}", url.replace("&amp;", "&"));
    let response = http.get(url).send().await.ok()?.error_for_status().ok()?;
    let mime = response.headers().get(header::CONTENT_TYPE)?.to_str().ok()?.to_string();
    if !mime.starts_with("image/") || response.content_length().is_some_and(|l| l as usize > MAX_IMAGE_BYTES) {
        return None;
    }
    let bytes = response.bytes().await.ok()?;
    (bytes.len() <= MAX_IMAGE_BYTES).then(|| (mime, bytes.to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_wikimedia_images_once() {
        let html = r#"<img alt="a" src="//upload.wikimedia.org/a/b/c.jpg" srcset="//upload.wikimedia.org/a/b/c2.jpg 2x">
            <img src="//upload.wikimedia.org/a/b/c.jpg"><img src="https://evil.example/x.png"><img src="/static/logo.svg">
            <img class="mw-file-element" src="//thumb.wikimedia.org/x/20px-y.png?a=b&amp;c=d">"#;
        assert_eq!(
            image_urls(html),
            vec!["//upload.wikimedia.org/a/b/c.jpg", "//thumb.wikimedia.org/x/20px-y.png?a=b&amp;c=d"]
        );
    }
}
