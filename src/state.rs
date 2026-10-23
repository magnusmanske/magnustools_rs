//! What every request handler can use. Shared via `Arc`.

use crate::config::Config;
use anyhow::Result;

/// Sent with every outbound request, per the Wikimedia User-Agent policy.
const USER_AGENT: &str =
    wikimisc::user_agent!(name = "magnustools", description = "https://magnustools.toolforge.org/");

#[derive(Debug)]
pub struct AppState {
    pub config: Config,
    pub http: reqwest::Client,
}

impl AppState {
    pub fn new(config: Config) -> Result<Self> {
        Ok(Self { config, http: wikimisc::http::client(USER_AGENT)? })
    }
}
