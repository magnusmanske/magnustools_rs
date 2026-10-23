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
    /// The PHP tool's `public_html`, whose static files are served as they are.
    /// Relative paths are relative to the config file.
    pub public_html: PathBuf,
    /// Database credentials (`replica.my.cnf`, as Toolforge provides it). With
    /// `local='true'` in it, connections go to SSH tunnels instead.
    #[serde(default = "default_replica_credentials")]
    pub replica_credentials: PathBuf,
    #[serde(default)]
    pub tool_db: ToolDbConfig,
    /// The Flickr API key, alone in a file; without it, `flickrfree.php` is unavailable.
    #[serde(default)]
    pub flickr_key_file: Option<PathBuf>,
    /// Service dashboard push; the token may also come from `$DASHBOARD_TOKEN`.
    #[serde(default)]
    pub telemetry: TelemetryConfig,
}

fn default_replica_credentials() -> PathBuf {
    "replica.my.cnf".into()
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("cannot read config file {}", path.display()))?;
        let mut config: Self =
            serde_json::from_str(&text).with_context(|| format!("invalid config file {}", path.display()))?;
        let dir = path.parent().unwrap_or(Path::new("."));
        config.public_html = dir.join(&config.public_html);
        config.flickr_key_file = config.flickr_key_file.map(|f| dir.join(f));
        config.replica_credentials = dir.join(&config.replica_credentials);
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
        assert_eq!(config.public_html, dir.path().join("php/public_html"));
        assert_eq!(config.replica_credentials, dir.path().join("replica.my.cnf"));
        assert!(config.telemetry.token.is_none());
    }

    #[test]
    fn absolute_paths_stay() {
        let (config, _dir) = load(r#"{"public_html": "/data/project/x/public_html"}"#);
        assert_eq!(config.unwrap().public_html, Path::new("/data/project/x/public_html"));
    }

    #[test]
    fn typos_and_omissions_are_errors() {
        assert!(load(r#"{"public_html": "x", "telemtry": {}}"#).0.is_err());
        assert!(load("{}").0.is_err());
    }
}
