//! Backend control commands for Rancher Desktop.
//!
//! Provides start, stop, restart, and status commands that interact with
//! the /`v1/backend_state` API endpoint.

use crate::cli::Cli;
use crate::client::http::{build_client, HttpClientConfig};
use crate::config::RdEngineConfig;
use anyhow::{Context, Result};
use colored::Colorize;
use serde::Serialize;
use tracing::{debug, info};

/// Backend states as returned by the Rancher Desktop API
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum BackendState {
    Started,
    Starting,
    Stopped,
    Stopping,
    Error,
    Disabled,
    #[serde(other)]
    Unknown,
}

impl std::fmt::Display for BackendState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Started => write!(f, "STARTED"),
            Self::Starting => write!(f, "STARTING"),
            Self::Stopped => write!(f, "STOPPED"),
            Self::Stopping => write!(f, "STOPPING"),
            Self::Error => write!(f, "ERROR"),
            Self::Disabled => write!(f, "DISABLED"),
            Self::Unknown => write!(f, "UNKNOWN"),
        }
    }
}

impl BackendState {
    /// Parse state from API response string
    fn from_str(s: &str) -> Self {
        match s.trim().trim_matches('"').to_uppercase().as_str() {
            "STARTED" => Self::Started,
            "STARTING" => Self::Starting,
            "STOPPED" => Self::Stopped,
            "STOPPING" => Self::Stopping,
            "ERROR" => Self::Error,
            "DISABLED" => Self::Disabled,
            _ => Self::Unknown,
        }
    }

    /// Get colored status string for display
    fn colored(&self) -> colored::ColoredString {
        match self {
            Self::Started => "STARTED".green(),
            Self::Starting => "STARTING".yellow(),
            Self::Stopped => "STOPPED".red(),
            Self::Stopping => "STOPPING".yellow(),
            Self::Error => "ERROR".red().bold(),
            Self::Disabled => "DISABLED".dimmed(),
            Self::Unknown => "UNKNOWN".dimmed(),
        }
    }
}

/// Status output structure
#[derive(Debug, Serialize)]
pub struct StatusOutput {
    pub state: BackendState,
    pub api_endpoint: String,
}

/// Start the Rancher Desktop backend
pub async fn start(cli: &Cli) -> Result<()> {
    info!("Starting Rancher Desktop backend");
    set_backend_state(cli, "STARTED", "Starting").await
}

/// Stop the Rancher Desktop backend
pub async fn stop(cli: &Cli) -> Result<()> {
    info!("Stopping Rancher Desktop backend");
    set_backend_state(cli, "STOPPED", "Stopping").await
}

/// Restart the Rancher Desktop backend
pub async fn restart(cli: &Cli) -> Result<()> {
    info!("Restarting Rancher Desktop backend");

    // First stop, then start
    set_backend_state(cli, "STOPPED", "Stopping").await?;

    // Wait a moment for the backend to stop
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    set_backend_state(cli, "STARTED", "Starting").await
}

/// Show the backend status
pub async fn status(cli: &Cli) -> Result<()> {
    info!("Checking backend status");

    let config = RdEngineConfig::load()
        .context("Failed to load Rancher Desktop configuration. Is Rancher Desktop running?")?;

    let state = get_backend_state(&config, cli).await?;

    if cli.json {
        let output = StatusOutput {
            state,
            api_endpoint: config.api_base_url(),
        };
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!("Backend status: {}", state.colored());
    }

    Ok(())
}

/// Get the current backend state
async fn get_backend_state(config: &RdEngineConfig, cli: &Cli) -> Result<BackendState> {
    let client_config = HttpClientConfig::with_timeout(cli.insecure, cli.timeout);
    let client = build_client(&client_config)?;

    let url = config.api_url("/v1/backend_state");
    debug!("Getting backend state from {}", url);

    let response = client
        .get(&url)
        .header("Authorization", config.basic_auth())
        .send()
        .await
        .context("Failed to get backend state")?;

    if !response.status().is_success() {
        anyhow::bail!("Failed to get backend state: HTTP {}", response.status());
    }

    let body = response
        .text()
        .await
        .context("Failed to read response body")?;

    Ok(BackendState::from_str(&body))
}

/// Set the backend state via PUT request
async fn set_backend_state(cli: &Cli, target_state: &str, action: &str) -> Result<()> {
    let config = RdEngineConfig::load()
        .context("Failed to load Rancher Desktop configuration. Is Rancher Desktop running?")?;

    let client_config = HttpClientConfig::with_timeout(cli.insecure, cli.timeout);
    let client = build_client(&client_config)?;

    let url = config.api_url("/v1/backend_state");
    debug!("Setting backend state to {} via {}", target_state, url);

    if !cli.quiet && !cli.json {
        println!("{action} Rancher Desktop backend...");
    }

    let response = client
        .put(&url)
        .header("Authorization", config.basic_auth())
        .header("Content-Type", "application/json")
        .body(format!("\"{target_state}\""))
        .send()
        .await
        .context("Failed to set backend state")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Failed to set backend state: HTTP {status} - {body}");
    }

    // Get the new state
    let new_state = get_backend_state(&config, cli).await?;

    if cli.json {
        let output = StatusOutput {
            state: new_state,
            api_endpoint: config.api_base_url(),
        };
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else if !cli.quiet {
        println!("Backend status: {}", new_state.colored());
    }

    Ok(())
}
