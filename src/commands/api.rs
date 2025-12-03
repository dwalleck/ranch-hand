//! Generic API command for interacting with the Rancher Desktop HTTP API.
//!
//! Provides direct access to any API endpoint with support for all HTTP methods,
//! JSON request bodies, and pretty-printed or raw output.

use crate::cli::{Cli, HttpMethod};
use crate::client::http::{build_client, HttpClientConfig};
use crate::config::RdEngineConfig;
use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;
use tracing::{debug, info};

/// Run the api command
pub async fn run(
    cli: &Cli,
    endpoint: &str,
    method: HttpMethod,
    body: Option<String>,
    input: Option<PathBuf>,
    raw: bool,
) -> Result<()> {
    info!("API request: {} {}", method, endpoint);

    // Load configuration
    let config = RdEngineConfig::load()
        .context("Failed to load Rancher Desktop configuration. Is Rancher Desktop running?")?;

    // Build the URL
    let url = config.api_url(endpoint);
    debug!("Full URL: {}", url);

    // Get request body from --body or --input
    let request_body = get_request_body(body, input)?;

    // Build HTTP client
    let client_config = HttpClientConfig::with_timeout(cli.insecure, cli.timeout);
    let client = build_client(&client_config)?;

    // Build and send the request
    let mut request = match method {
        HttpMethod::Get => client.get(&url),
        HttpMethod::Post => client.post(&url),
        HttpMethod::Put => client.put(&url),
        HttpMethod::Delete => client.delete(&url),
    };

    // Add authorization header
    request = request.header("Authorization", config.basic_auth());

    // Add body if provided
    if let Some(body) = &request_body {
        debug!("Request body: {}", body);
        request = request
            .header("Content-Type", "application/json")
            .body(body.clone());
    }

    // Send the request
    let response = request.send().await.context("Failed to send API request")?;

    let status = response.status();
    debug!("Response status: {}", status);

    // Get response body
    let response_body = response
        .text()
        .await
        .context("Failed to read response body")?;

    // Output the response
    if raw || cli.quiet {
        // Raw output - just print as-is
        if !response_body.is_empty() {
            println!("{response_body}");
        }
    } else {
        // Pretty print JSON if possible
        match serde_json::from_str::<serde_json::Value>(&response_body) {
            Ok(json) => {
                println!("{}", serde_json::to_string_pretty(&json)?);
            }
            Err(_) => {
                // Not JSON, print as-is
                if !response_body.is_empty() {
                    println!("{response_body}");
                }
            }
        }
    }

    // Return error if status is not success
    if !status.is_success() {
        anyhow::bail!("API request failed with status: {}", status);
    }

    Ok(())
}

/// Get request body from --body flag or --input file
fn get_request_body(body: Option<String>, input: Option<PathBuf>) -> Result<Option<String>> {
    match (body, input) {
        (Some(_), Some(_)) => {
            anyhow::bail!("Cannot specify both --body and --input");
        }
        (Some(body), None) => Ok(Some(body)),
        (None, Some(path)) => {
            let content = fs::read_to_string(&path)
                .with_context(|| format!("Failed to read input file: {}", path.display()))?;
            Ok(Some(content))
        }
        (None, None) => Ok(None),
    }
}
