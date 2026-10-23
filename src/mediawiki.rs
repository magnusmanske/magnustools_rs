//! The few MediaWiki API calls the tools make.

use crate::wiki::Wiki;
use anyhow::{Context, Result};
use serde_json::Value;

/// A GET request to the action API; `format=json&formatversion=2` is added.
pub async fn api(http: &reqwest::Client, wiki: &Wiki, params: &[(&str, &str)]) -> Result<Value> {
    http.get(wiki.api_url())
        .query(params)
        .query(&[("format", "json"), ("formatversion", "2")])
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .with_context(|| format!("API request to {wiki} failed"))?
        .json()
        .await
        .with_context(|| format!("API response from {wiki} is not JSON"))
}

/// The current wikitext of a page, following redirects; `None` if there is no such page.
pub async fn page_wikitext(http: &reqwest::Client, wiki: &Wiki, title: &str) -> Result<Option<String>> {
    let params = [
        ("action", "query"),
        ("prop", "revisions"),
        ("rvprop", "content"),
        ("rvslots", "main"),
        ("redirects", "1"),
        ("titles", title),
    ];
    let json = api(http, wiki, &params).await?;
    Ok(json["query"]["pages"][0]["revisions"][0]["slots"]["main"]["content"].as_str().map(str::to_string))
}

/// A page rendered to HTML (`action=render`); `None` if there is no such page.
pub async fn page_html(http: &reqwest::Client, wiki: &Wiki, title: &str) -> Result<Option<String>> {
    let response = http
        .get(wiki.index_url())
        .query(&[("action", "render"), ("title", title)])
        .send()
        .await
        .with_context(|| format!("cannot render {title} on {wiki}"))?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let html = response.error_for_status()?.text().await?;
    Ok(Some(html))
}
