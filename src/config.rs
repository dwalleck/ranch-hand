use crate::paths;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::fs;
use std::path::PathBuf;
use thiserror::Error;

/// Default port for the Rancher Desktop API
pub const DEFAULT_API_PORT: u16 = 6107;

/// Default host for the Rancher Desktop API
pub const DEFAULT_API_HOST: &str = "127.0.0.1";

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("rd-engine.json not found - is Rancher Desktop running?")]
    NotFound,
    #[error("Failed to read rd-engine.json: {0}")]
    ReadError(String),
    #[error("Failed to parse rd-engine.json: {0}")]
    ParseError(String),
}

/// Credentials and connection info from rd-engine.json
#[derive(Debug, Clone, Deserialize)]
pub struct RdEngineConfig {
    pub user: String,
    pub password: String,
    #[serde(default = "default_host")]
    pub host: String,
    pub port: u16,
}

fn default_host() -> String {
    DEFAULT_API_HOST.to_string()
}

impl Default for RdEngineConfig {
    fn default() -> Self {
        Self {
            user: String::new(),
            password: String::new(),
            host: default_host(),
            port: DEFAULT_API_PORT,
        }
    }
}

impl RdEngineConfig {
    /// Load configuration from rd-engine.json
    pub fn load() -> Result<Self, ConfigError> {
        let path = paths::rd_engine_json_path().map_err(|e| ConfigError::ReadError(e.to_string()))?;
        Self::load_from_path(&path)
    }

    /// Load configuration from a specific path
    pub fn load_from_path(path: &PathBuf) -> Result<Self, ConfigError> {
        if !path.exists() {
            return Err(ConfigError::NotFound);
        }

        let contents =
            fs::read_to_string(path).map_err(|e| ConfigError::ReadError(e.to_string()))?;

        serde_json::from_str(&contents).map_err(|e| ConfigError::ParseError(e.to_string()))
    }

    /// Try to load config, returning None if not found (Rancher Desktop not running)
    pub fn try_load() -> Option<Self> {
        Self::load().ok()
    }

    /// Get the base URL for the Rancher Desktop API
    pub fn api_base_url(&self) -> String {
        format!("http://{}:{}", self.host, self.port)
    }

    /// Get the full URL for an API endpoint
    pub fn api_url(&self, endpoint: &str) -> String {
        let endpoint = endpoint.trim_start_matches('/');
        format!("{}/{}", self.api_base_url(), endpoint)
    }

    /// Get basic auth header value
    pub fn basic_auth(&self) -> String {
        use base64::Engine;
        let credentials = format!("{}:{}", self.user, self.password);
        let encoded = base64::engine::general_purpose::STANDARD.encode(credentials);
        format!("Basic {}", encoded)
    }
}

/// Runtime configuration combining file config with CLI overrides
#[derive(Debug, Clone)]
pub struct AppConfig {
    /// Loaded or constructed rd-engine config
    pub rd_engine: Option<RdEngineConfig>,
    /// Accept invalid SSL certificates
    pub insecure: bool,
    /// Output in JSON format
    pub json: bool,
    /// Verbosity level
    pub verbose: u8,
    /// Suppress output
    pub quiet: bool,
}

impl AppConfig {
    /// Create config from CLI arguments
    pub fn from_cli(cli: &crate::cli::Cli) -> Self {
        let rd_engine = if let Some(config_path) = &cli.config {
            RdEngineConfig::load_from_path(config_path).ok()
        } else {
            RdEngineConfig::try_load()
        };

        Self {
            rd_engine,
            insecure: cli.insecure,
            json: cli.json,
            verbose: cli.verbose,
            quiet: cli.quiet,
        }
    }

    /// Check if Rancher Desktop API is available
    pub fn has_api_config(&self) -> bool {
        self.rd_engine.is_some()
    }

    /// Get the rd-engine config, returning an error if not available
    pub fn require_api_config(&self) -> Result<&RdEngineConfig> {
        self.rd_engine
            .as_ref()
            .context("rd-engine.json not found - is Rancher Desktop running?")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn create_temp_config(content: &str) -> NamedTempFile {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(content.as_bytes()).unwrap();
        file
    }

    #[test]
    fn test_load_valid_config() {
        let content = r#"{
            "user": "admin",
            "password": "secret123",
            "port": 6107
        }"#;
        let file = create_temp_config(content);

        let config = RdEngineConfig::load_from_path(&file.path().to_path_buf()).unwrap();
        assert_eq!(config.user, "admin");
        assert_eq!(config.password, "secret123");
        assert_eq!(config.port, 6107);
        assert_eq!(config.host, "127.0.0.1"); // default
    }

    #[test]
    fn test_load_with_host() {
        let content = r#"{
            "user": "admin",
            "password": "secret123",
            "host": "localhost",
            "port": 8080
        }"#;
        let file = create_temp_config(content);

        let config = RdEngineConfig::load_from_path(&file.path().to_path_buf()).unwrap();
        assert_eq!(config.host, "localhost");
        assert_eq!(config.port, 8080);
    }

    #[test]
    fn test_api_url() {
        let config = RdEngineConfig {
            user: "admin".to_string(),
            password: "secret".to_string(),
            host: "127.0.0.1".to_string(),
            port: DEFAULT_API_PORT,
        };

        assert_eq!(config.api_base_url(), "http://127.0.0.1:6107");
        assert_eq!(
            config.api_url("/v1/settings"),
            "http://127.0.0.1:6107/v1/settings"
        );
        assert_eq!(
            config.api_url("v1/settings"),
            "http://127.0.0.1:6107/v1/settings"
        );
        // Handle multiple leading slashes
        assert_eq!(
            config.api_url("///v1/settings"),
            "http://127.0.0.1:6107/v1/settings"
        );
    }

    #[test]
    fn test_basic_auth() {
        let config = RdEngineConfig {
            user: "admin".to_string(),
            password: "secret".to_string(),
            host: "127.0.0.1".to_string(),
            port: DEFAULT_API_PORT,
        };

        // base64("admin:secret") = "YWRtaW46c2VjcmV0"
        assert_eq!(config.basic_auth(), "Basic YWRtaW46c2VjcmV0");
    }

    #[test]
    fn test_config_not_found() {
        let result =
            RdEngineConfig::load_from_path(&PathBuf::from("/nonexistent/path/rd-engine.json"));
        assert!(matches!(result, Err(ConfigError::NotFound)));
    }

    #[test]
    fn test_invalid_json() {
        let content = "not valid json";
        let file = create_temp_config(content);

        let result = RdEngineConfig::load_from_path(&file.path().to_path_buf());
        assert!(matches!(result, Err(ConfigError::ParseError(_))));
    }
}
