//! Version command for displaying version information.
//!
//! Shows ranch-hand version and Rancher Desktop configuration details
//! when available.

use crate::cli::Cli;
use crate::client::http::{build_client, HttpClientConfig};
use crate::config::RdEngineConfig;
use anyhow::Result;
use colored::Colorize;
use serde::Serialize;
use tracing::debug;

/// Version information output structure
#[derive(Debug, Serialize)]
pub struct VersionInfo {
    /// ranch-hand CLI version
    pub ranch_hand: String,
    /// Rancher Desktop info (if available)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rancher_desktop: Option<RancherDesktopInfo>,
}

/// Rancher Desktop version and configuration info
#[derive(Debug, Serialize)]
pub struct RancherDesktopInfo {
    /// Kubernetes version
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kubernetes_version: Option<String>,
    /// Container engine (containerd or moby)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container_engine: Option<String>,
    /// Whether Kubernetes is enabled
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kubernetes_enabled: Option<bool>,
    /// API endpoint
    pub api_endpoint: String,
}

/// Run the version command
pub async fn run(cli: &Cli) -> Result<()> {
    let ranch_hand_version = env!("CARGO_PKG_VERSION").to_string();

    // Try to get Rancher Desktop info
    let rd_info = get_rancher_desktop_info(cli).await;

    if cli.json {
        let output = VersionInfo {
            ranch_hand: ranch_hand_version,
            rancher_desktop: rd_info,
        };
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!("{}", "ranch-hand".bold().cyan());
        println!("  Version: {ranch_hand_version}");
        println!();

        if let Some(rd) = rd_info {
            println!("{}", "Rancher Desktop".bold().cyan());
            println!("  API: {}", rd.api_endpoint);

            if let Some(k8s) = rd.kubernetes_version {
                println!("  Kubernetes: {k8s}");
            }
            if let Some(enabled) = rd.kubernetes_enabled {
                println!(
                    "  Kubernetes enabled: {}",
                    if enabled { "yes" } else { "no" }
                );
            }
            if let Some(engine) = rd.container_engine {
                println!("  Container engine: {engine}");
            }
        } else {
            println!("{}", "Rancher Desktop".bold().dimmed());
            println!("  {}", "Not running or not accessible".dimmed());
        }
    }

    Ok(())
}

/// Try to get Rancher Desktop version and configuration info
async fn get_rancher_desktop_info(cli: &Cli) -> Option<RancherDesktopInfo> {
    let config = RdEngineConfig::load().ok()?;
    let api_endpoint = config.api_base_url();

    // Try to fetch settings from the API
    let client_config = HttpClientConfig::with_timeout(cli.insecure, cli.timeout);
    let client = build_client(&client_config).ok()?;

    let url = config.api_url("/v1/settings");
    debug!("Fetching settings from {}", url);

    let response = client
        .get(&url)
        .header("Authorization", config.basic_auth())
        .send()
        .await
        .ok()?;

    if !response.status().is_success() {
        debug!("Settings request failed: {}", response.status());
        return Some(RancherDesktopInfo {
            kubernetes_version: None,
            container_engine: None,
            kubernetes_enabled: None,
            api_endpoint,
        });
    }

    let settings: serde_json::Value = response.json().await.ok()?;

    let kubernetes_version = settings
        .get("kubernetes")
        .and_then(|k| k.get("version"))
        .and_then(|v| v.as_str())
        .map(std::string::ToString::to_string);

    let kubernetes_enabled = settings
        .get("kubernetes")
        .and_then(|k| k.get("enabled"))
        .and_then(serde_json::Value::as_bool);

    let container_engine = settings
        .get("containerEngine")
        .and_then(|c| c.get("name"))
        .and_then(|n| n.as_str())
        .map(std::string::ToString::to_string);

    Some(RancherDesktopInfo {
        kubernetes_version,
        container_engine,
        kubernetes_enabled,
        api_endpoint,
    })
}
