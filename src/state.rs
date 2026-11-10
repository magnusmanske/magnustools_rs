//! What every request handler can use. Shared via `Arc`.

use crate::cache::TimedMap;
use crate::config::Config;
use crate::mediawiki::Api;
use crate::oauth::{Consumer, OAuth};
use crate::replicas::Replicas;
use crate::tool_db::ToolDb;
use crate::wiki::Wiki;
use anyhow::{Context, Result};
use axum::http::HeaderMap;
use axum_extra::extract::PrivateCookieJar;
use axum_extra::extract::cookie::Key;
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// Sent with every outbound request, per the Wikimedia User-Agent policy.
pub const USER_AGENT: &str =
    wikimisc::user_agent!(name = "magnustools", description = "https://magnustools.toolforge.org/");

/// Site info and namespace names change rarely, and are only loaded once per wiki.
const API_CACHE_FOR: Duration = Duration::from_secs(86_400);
/// Wikis to keep API clients for; a request rarely touches more than a few.
const API_CACHE_WIKIS: usize = 128;
/// The write check only needs the user name; cache it briefly per access token.
const USER_CACHE_FOR: Duration = Duration::from_secs(300);
const USER_CACHE_USERS: usize = 64;
/// The dashboard is polled every minute; a few seconds of sharing is enough.
const DASHBOARD_CACHE_FOR: Duration = Duration::from_secs(30);
/// The usage overview changes once a day; one minute is plenty.
const USAGE_CACHE_FOR: Duration = Duration::from_secs(60);

pub struct AppState {
    pub config: Config,
    pub http: reqwest::Client,
    pub replicas: Replicas,
    pub tool_db: ToolDb,
    pub cookie_key: Key,
    pub buggregator_oauth: Option<OAuth>,
    pub uploader_oauth: Option<OAuth>,
    /// API clients with each wiki's cached site info.
    apis: TimedMap<String, Api>,
    /// Access-token hash => MediaWiki user name, for the write check.
    pub user_names: TimedMap<String, String>,
    /// The dashboard overview, with the time it was computed.
    pub dash_overview: TimedMap<String, (i64, Value)>,
    /// The usage overview, by range in days.
    pub usage_overview: TimedMap<String, Value>,
}

impl AppState {
    pub fn new(config: Config) -> Result<Self> {
        let http = wikimisc::http::client(USER_AGENT)?;
        let cookie_key = match &config.cookie_key {
            Some(hex) => Key::try_from(decode_hex(hex)?.as_slice()).context("cookie_key must be 64 bytes")?,
            None => {
                tracing::warn!("no cookie_key configured: logins last until the next restart");
                Key::generate()
            }
        };
        Ok(Self {
            replicas: Replicas::new(&config.replica_credentials)?,
            tool_db: ToolDb::new(&config.tool_db, &config.replica_credentials)?,
            buggregator_oauth: oauth(&config.buggregator_oauth, &http),
            uploader_oauth: oauth(&config.uploader_oauth, &http),
            cookie_key,
            http,
            config,
            apis: TimedMap::new(API_CACHE_FOR, API_CACHE_WIKIS),
            user_names: TimedMap::new(USER_CACHE_FOR, USER_CACHE_USERS),
            dash_overview: TimedMap::new(DASHBOARD_CACHE_FOR, 4),
            usage_overview: TimedMap::new(USAGE_CACHE_FOR, 8),
        })
    }

    /// The API client for `wiki`, with its site info cached. Queries use the
    /// modern (`formatversion=2`) JSON shape; site info stays on version 1
    /// internally, for the namespace accessors and `Title`.
    pub async fn api(&self, wiki: &Wiki) -> Result<Arc<Api>> {
        let api_url = wiki.api_url();
        let http = self.http.clone();
        self.apis
            .get_or_fetch(wiki.dbname().to_string(), async move || {
                let mut api = Api::new_from_client_with_user_agent(&api_url, http, USER_AGENT)
                    .await
                    .map_err(anyhow::Error::from)?;
                api.set_formatversion(2);
                Ok(api)
            })
            .await
    }
}

impl AppState {
    /// The request's encrypted cookies.
    pub fn cookies(&self, headers: &HeaderMap) -> PrivateCookieJar {
        PrivateCookieJar::from_headers(headers, self.cookie_key.clone())
    }
}

fn oauth(ini: &Path, http: &reqwest::Client) -> Option<OAuth> {
    match Consumer::from_ini(ini) {
        Ok(consumer) => Some(OAuth::new(consumer, http.clone())),
        Err(e) => {
            tracing::warn!("OAuth unavailable: {e:#}");
            None
        }
    }
}

fn decode_hex(hex: &str) -> Result<Vec<u8>> {
    let hex = hex.trim();
    (0..hex.len())
        .step_by(2)
        .map(|i| hex.get(i..i + 2).and_then(|b| u8::from_str_radix(b, 16).ok()).context("cookie_key is not hex"))
        .collect()
}
