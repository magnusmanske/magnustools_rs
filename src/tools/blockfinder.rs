//! `blockfinder.php`: the global blocks, and the local blocks on one wiki,
//! that apply to an IP address. Unlike the original, local blocks are only
//! checked on the chosen wiki; scanning every wiki took minutes and was
//! rarely worth it.

use crate::mediawiki;
use crate::state::AppState;
use crate::web::{AppError, Params, render};
use crate::wiki::Wiki;
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use serde::Deserialize;
use std::net::IpAddr;
use std::str::FromStr;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default = "default_language", alias = "language")]
    lang: String,
    #[serde(default = "default_project", alias = "project")]
    family: String,
    #[serde(default)]
    ip: String,
}

fn default_language() -> String {
    "en".to_string()
}

fn default_project() -> String {
    "wikipedia".to_string()
}

struct Block {
    time: String,
    by: String,
    by_url: String,
    target: String,
    target_url: String,
    options: String,
    reason: String,
    expiry: String,
}

struct Results {
    global: Vec<Block>,
    local: Vec<Block>,
    wiki: String,
}

#[derive(Template)]
#[template(path = "tools/blockfinder.html")]
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
    let ip = request.ip.trim();
    let mut page = Page { title: "Block finder [Erwin85]", request: &request, results: None, error: None };
    if ip.is_empty() {
        return render(&page);
    }
    if IpAddr::from_str(ip).is_err() {
        page.error = Some(format!("“{ip}” is not an IP address."));
        return render(&page);
    }
    let wiki = Wiki::new(&request.lang, &request.family)?;
    page.results = Some(results(&state, &wiki, ip).await?);
    render(&page)
}

async fn results(state: &AppState, wiki: &Wiki, ip: &str) -> Result<Results, AppError> {
    let global = global_blocks(state, ip).await?;
    let local = local_blocks(state, wiki, ip).await?;
    Ok(Results { global, local, wiki: wiki.to_string() })
}

async fn global_blocks(state: &AppState, ip: &str) -> Result<Vec<Block>, AppError> {
    let meta = Wiki::new("meta", "wikimedia")?;
    let json = mediawiki::api(
        &state.http,
        &meta,
        &[("action", "query"), ("list", "globalblocks"), ("bgip", ip), ("bglimit", "500")],
    )
    .await?;
    let meta_url = meta.page_url("").trim_end_matches('/').to_string();
    Ok(blocks(json, "globalblocks")
        .into_iter()
        .map(|block| {
            let target = field(&block, "target").or_else(|| field(&block, "address")).unwrap_or_default();
            let by = field(&block, "by").unwrap_or_default();
            let by_url = match field(&block, "bywiki").and_then(|w| Wiki::from_dbname(&w).ok()) {
                Some(wiki) => wiki.page_url(&format!("User:{by}")),
                None => meta.page_url(&format!("User:{by}")),
            };
            Block {
                time: display_time(&field(&block, "timestamp").unwrap_or_default()),
                by,
                by_url,
                target_url: format!("{meta_url}/Special:GlobalBlockList?ip={}", crate::wiki::percent_encode(&target)),
                target,
                options: options(&block),
                reason: field(&block, "reason").unwrap_or_default(),
                expiry: display_time(&field(&block, "expiry").unwrap_or_default()),
            }
        })
        .collect())
}

async fn local_blocks(state: &AppState, wiki: &Wiki, ip: &str) -> Result<Vec<Block>, AppError> {
    let json =
        mediawiki::api(&state.http, wiki, &[("action", "query"), ("list", "blocks"), ("bkip", ip), ("bklimit", "500")])
            .await?;
    Ok(blocks(json, "blocks")
        .into_iter()
        .map(|block| {
            let target = field(&block, "user").unwrap_or_default();
            let by = field(&block, "by").unwrap_or_default();
            Block {
                time: display_time(&field(&block, "timestamp").unwrap_or_default()),
                by_url: wiki.page_url(&format!("User:{by}")),
                by,
                target_url: format!(
                    "https://{}/wiki/Special:BlockList?ip={}",
                    wiki.host(),
                    crate::wiki::percent_encode(&target)
                ),
                target,
                options: options(&block),
                reason: field(&block, "reason").unwrap_or_default(),
                expiry: display_time(&field(&block, "expiry").unwrap_or_default()),
            }
        })
        .collect())
}

/// The entries of `key` in a query API response.
fn blocks(json: serde_json::Value, key: &str) -> Vec<serde_json::Value> {
    json["query"][key].as_array().cloned().unwrap_or_default()
}

fn field(block: &serde_json::Value, key: &str) -> Option<String> {
    block[key].as_str().map(str::to_string)
}

/// The flags an API block has, as words.
fn options(block: &serde_json::Value) -> String {
    let flag = |key: &str| block.get(key).and_then(serde_json::Value::as_bool).unwrap_or(false);
    let mut options = vec![];
    if flag("anononly") {
        options.push("anonymous users only");
    }
    if flag("nocreate") || flag("account-creation-disabled") {
        options.push("account creation blocked");
    }
    if flag("autoblock") || flag("autoblocking-enabled") {
        options.push("autoblock enabled");
    }
    options.join(", ")
}

/// An API timestamp (`2010-09-19T02:59:51Z`) as `2010-09-19 02:59:51 (UTC)`.
fn display_time(value: &str) -> String {
    match value.get(..19) {
        Some(time) if value.len() == 20 && value.ends_with('Z') => format!("{} (UTC)", time.replace('T', " ")),
        _ => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn times() {
        assert_eq!(display_time("2024-01-02T03:04:05Z"), "2024-01-02 03:04:05 (UTC)");
        assert_eq!(display_time("infinity"), "infinity");
    }

    #[test]
    fn flags() {
        assert_eq!(options(&json!({"nocreate": true})), "account creation blocked");
        assert_eq!(options(&json!({"anononly": true})), "anonymous users only");
        assert_eq!(options(&json!({"anononly": false, "nocreate": false})), "");
        assert_eq!(options(&json!({})), "");
    }
}
