//! Typed configuration, loaded from a JSON file that is never committed.
//! See `config.json.template` for the shape.

use crate::tool_db::ToolDbConfig;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use wikimisc::telemetry::TelemetryConfig;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The PHP tool's `public_html`, whose static files are served as they
    /// are: the libraries other tools load (`resources/`) and retired pages.
    /// The tools here need none of it. Relative paths, here and below, are
    /// relative to the config file.
    #[serde(default)]
    pub public_html: Option<PathBuf>,
    /// Database credentials (`replica.my.cnf`, as Toolforge provides it). With
    /// `local='true'` in it, connections go to SSH tunnels instead.
    #[serde(default = "default_replica_credentials")]
    pub replica_credentials: PathBuf,
    #[serde(default)]
    pub tool_db: ToolDbConfig,
    /// Where this service is reachable, for OAuth callbacks.
    #[serde(default = "default_public_url")]
    pub public_url: String,
    /// Encrypts the login cookies: 64 bytes, hex. Without one, a random key
    /// is used, so logins last only until a restart, and only on one replica.
    #[serde(default)]
    pub cookie_key: Option<String>,
    /// OAuth consumers (`consumerKey`, `consumerSecret`, `agent`); without
    /// the file, logging in to that part is unavailable.
    #[serde(default = "default_buggregator_oauth")]
    pub buggregator_oauth: PathBuf,
    #[serde(default = "default_uploader_oauth")]
    pub uploader_oauth: PathBuf,
    /// The Flickr API key, alone in a file; without it, `flickrfree.php` is unavailable.
    #[serde(default)]
    pub flickr_key_file: Option<PathBuf>,
    #[serde(default)]
    pub monitor: MonitorConfig,
    /// Service dashboard push; the token may also come from `$DASHBOARD_TOKEN`.
    #[serde(default)]
    pub telemetry: TelemetryConfig,
}

/// The `monitor` job: checks tools, writes the status page, and mails new alerts.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MonitorConfig {
    /// Without an address, alerts are only logged.
    #[serde(default)]
    pub mail_to: Option<String>,
    #[serde(default = "default_mail_from")]
    pub mail_from: String,
    #[serde(default = "default_smtp_host")]
    pub smtp_host: String,
    /// The last alert mail, so that an unchanged situation is mailed only once.
    #[serde(default = "default_monitor_state")]
    pub state_file: PathBuf,
    /// The dashboard services known to be offline, so that each outage is mailed only once.
    #[serde(default = "default_dashboard_state")]
    pub dashboard_state_file: PathBuf,
    /// The latest results, as a page; served as `/tool_status.html`.
    #[serde(default = "default_status_page")]
    pub status_page: PathBuf,
}

impl Default for MonitorConfig {
    fn default() -> Self {
        Self {
            mail_to: None,
            mail_from: default_mail_from(),
            smtp_host: default_smtp_host(),
            state_file: default_monitor_state(),
            dashboard_state_file: default_dashboard_state(),
            status_page: default_status_page(),
        }
    }
}

fn default_mail_from() -> String {
    "Tool monitor <tools.magnustools@toolforge.org>".to_string()
}

fn default_smtp_host() -> String {
    "mail.tools.wmcloud.org".to_string()
}

fn default_monitor_state() -> PathBuf {
    "monitor_last_message.html".into()
}

fn default_dashboard_state() -> PathBuf {
    "dashboard_alerts_state.txt".into()
}

fn default_status_page() -> PathBuf {
    "tool_status.html".into()
}

fn default_replica_credentials() -> PathBuf {
    "replica.my.cnf".into()
}

fn default_public_url() -> String {
    "https://magnustools.toolforge.org".to_string()
}

fn default_buggregator_oauth() -> PathBuf {
    "buggregator_oauth.ini".into()
}

fn default_uploader_oauth() -> PathBuf {
    "oauth.ini".into()
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("cannot read config file {}", path.display()))?;
        let mut config: Self =
            serde_json::from_str(&text).with_context(|| format!("invalid config file {}", path.display()))?;
        let dir = path.parent().unwrap_or(Path::new("."));
        config.public_html = config.public_html.map(|p| dir.join(p));
        config.flickr_key_file = config.flickr_key_file.map(|f| dir.join(f));
        config.replica_credentials = dir.join(&config.replica_credentials);
        config.buggregator_oauth = dir.join(&config.buggregator_oauth);
        config.uploader_oauth = dir.join(&config.uploader_oauth);
        config.monitor.state_file = dir.join(&config.monitor.state_file);
        config.monitor.dashboard_state_file = dir.join(&config.monitor.dashboard_state_file);
        config.monitor.status_page = dir.join(&config.monitor.status_page);
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Loads `json` as a config file; also returns the directory it was in.
    fn load(json: &str) -> (Result<Config>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, json).unwrap();
        (Config::load(&path), dir)
    }

    #[test]
    fn relative_paths_are_relative_to_the_config_file() {
        let (config, dir) = load(r#"{"public_html": "php/public_html"}"#);
        let config = config.unwrap();
        assert_eq!(config.public_html, Some(dir.path().join("php/public_html")));
        assert_eq!(config.replica_credentials, dir.path().join("replica.my.cnf"));
        assert_eq!(config.monitor.status_page, dir.path().join("tool_status.html"));
        assert!(config.telemetry.token.is_none());
    }

    #[test]
    fn absolute_paths_stay() {
        let (config, _dir) = load(r#"{"public_html": "/data/project/x/public_html"}"#);
        assert_eq!(config.unwrap().public_html.as_deref(), Some(Path::new("/data/project/x/public_html")));
    }

    #[test]
    fn typos_are_errors_but_everything_has_a_default() {
        assert!(load(r#"{"public_html": "x", "telemtry": {}}"#).0.is_err());
        assert!(load("{}").0.unwrap().public_html.is_none());
    }
}
