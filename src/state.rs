//! What every request handler can use. Shared via `Arc`.

use crate::config::Config;
use crate::oauth::{Consumer, OAuth};
use crate::replicas::Replicas;
use crate::tool_db::ToolDb;
use anyhow::{Context, Result};
use axum::http::HeaderMap;
use axum_extra::extract::PrivateCookieJar;
use axum_extra::extract::cookie::Key;
use std::path::Path;

/// Sent with every outbound request, per the Wikimedia User-Agent policy.
const USER_AGENT: &str =
    wikimisc::user_agent!(name = "magnustools", description = "https://magnustools.toolforge.org/");

#[derive(Debug)]
pub struct AppState {
    pub config: Config,
    pub http: reqwest::Client,
    pub replicas: Replicas,
    pub tool_db: ToolDb,
    pub cookie_key: Key,
    pub buggregator_oauth: Option<OAuth>,
    pub uploader_oauth: Option<OAuth>,
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
        })
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
