//! Settings command for viewing and modifying Rancher Desktop settings.
//!
//! Supports viewing all settings, getting specific values using dot notation,
//! setting values, and factory reset.

use crate::cli::Cli;
use crate::client::http::{build_client, HttpClientConfig};
use crate::config::RdEngineConfig;
use anyhow::{Context, Result};
use colored::Colorize;
use serde_json::Value;
use tracing::{debug, info};

/// Show all settings
pub async fn show_all(cli: &Cli) -> Result<()> {
    info!("Fetching all settings");

    let config = RdEngineConfig::load()
        .context("Failed to load Rancher Desktop configuration. Is Rancher Desktop running?")?;

    let settings = get_settings(&config, cli).await?;

    if cli.json || cli.quiet {
        println!("{}", serde_json::to_string_pretty(&settings)?);
    } else {
        println!("{}", "Rancher Desktop Settings".bold().cyan());
        println!("{}", "=".repeat(40));
        println!();
        print_settings_tree(&settings, 0);
    }

    Ok(())
}

/// Get a specific setting value
pub async fn get(cli: &Cli, path: &str) -> Result<()> {
    info!("Getting setting: {}", path);

    let config = RdEngineConfig::load()
        .context("Failed to load Rancher Desktop configuration. Is Rancher Desktop running?")?;

    let settings = get_settings(&config, cli).await?;

    // Navigate to the requested path
    let value =
        get_value_at_path(&settings, path).with_context(|| format!("Setting not found: {path}"))?;

    if cli.json || cli.quiet {
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        println!("{}: {}", path.bold(), format_value(value));
    }

    Ok(())
}

/// Set a setting value
pub async fn set(cli: &Cli, path: &str, value: &str) -> Result<()> {
    info!("Setting {} = {}", path, value);

    let config = RdEngineConfig::load()
        .context("Failed to load Rancher Desktop configuration. Is Rancher Desktop running?")?;

    // Parse the value as JSON (or treat as string)
    let json_value = parse_value(value);
    debug!("Parsed value: {:?}", json_value);

    // Get current settings
    let mut settings = get_settings(&config, cli).await?;

    // Set the value at the path
    set_value_at_path(&mut settings, path, json_value.clone())
        .with_context(|| format!("Failed to set value at path: {path}"))?;

    // First, propose the settings to validate
    let propose_result = propose_settings(&config, cli, &settings).await?;

    // Check if there are any errors in the proposal
    if let Some(errors) = propose_result.get("errors") {
        if !errors.is_null() && errors.is_object() {
            let errors_obj = errors.as_object().unwrap();
            if !errors_obj.is_empty() {
                anyhow::bail!(
                    "Invalid settings: {}",
                    serde_json::to_string_pretty(errors)?
                );
            }
        }
    }

    // Apply the settings
    put_settings(&config, cli, &settings).await?;

    if cli.json {
        let output = serde_json::json!({
            "path": path,
            "value": json_value,
            "success": true
        });
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else if !cli.quiet {
        println!(
            "{} {} = {}",
            "Set".green(),
            path.bold(),
            format_value(&json_value)
        );

        // Check if restart is required
        if let Some(restart) = propose_result.get("requiresRestart") {
            if restart.as_bool().unwrap_or(false) {
                println!();
                println!(
                    "{} Restart required for changes to take effect.",
                    "Note:".yellow().bold()
                );
            }
        }
    }

    Ok(())
}

/// Reset all settings to defaults (factory reset)
pub async fn reset(cli: &Cli) -> Result<()> {
    info!("Resetting settings to defaults");

    let config = RdEngineConfig::load()
        .context("Failed to load Rancher Desktop configuration. Is Rancher Desktop running?")?;

    let client_config = HttpClientConfig::with_timeout(cli.insecure, cli.timeout);
    let client = build_client(&client_config)?;

    let url = config.api_url("/v1/factory_reset");
    debug!("Factory reset via {}", url);

    if !cli.quiet && !cli.json {
        println!(
            "{}",
            "Resetting Rancher Desktop to factory defaults...".yellow()
        );
    }

    let response = client
        .put(&url)
        .header("Authorization", config.basic_auth())
        .send()
        .await
        .context("Failed to reset settings")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Failed to reset settings: HTTP {} - {}", status, body);
    }

    if cli.json {
        let output = serde_json::json!({
            "success": true,
            "message": "Settings reset to defaults"
        });
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else if !cli.quiet {
        println!("{}", "Settings reset to defaults.".green());
        println!();
        println!(
            "{} Rancher Desktop may need to restart.",
            "Note:".yellow().bold()
        );
    }

    Ok(())
}

/// Fetch settings from the API
async fn get_settings(config: &RdEngineConfig, cli: &Cli) -> Result<Value> {
    let client_config = HttpClientConfig::with_timeout(cli.insecure, cli.timeout);
    let client = build_client(&client_config)?;

    let url = config.api_url("/v1/settings");
    debug!("Fetching settings from {}", url);

    let response = client
        .get(&url)
        .header("Authorization", config.basic_auth())
        .send()
        .await
        .context("Failed to fetch settings")?;

    if !response.status().is_success() {
        anyhow::bail!("Failed to fetch settings: HTTP {}", response.status());
    }

    response
        .json()
        .await
        .context("Failed to parse settings JSON")
}

/// Update settings via PUT
async fn put_settings(config: &RdEngineConfig, cli: &Cli, settings: &Value) -> Result<()> {
    let client_config = HttpClientConfig::with_timeout(cli.insecure, cli.timeout);
    let client = build_client(&client_config)?;

    let url = config.api_url("/v1/settings");
    debug!("Updating settings at {}", url);

    let response = client
        .put(&url)
        .header("Authorization", config.basic_auth())
        .header("Content-Type", "application/json")
        .json(settings)
        .send()
        .await
        .context("Failed to update settings")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Failed to update settings: HTTP {} - {}", status, body);
    }

    Ok(())
}

/// Propose settings for validation
async fn propose_settings(config: &RdEngineConfig, cli: &Cli, settings: &Value) -> Result<Value> {
    let client_config = HttpClientConfig::with_timeout(cli.insecure, cli.timeout);
    let client = build_client(&client_config)?;

    let url = config.api_url("/v1/propose_settings");
    debug!("Proposing settings at {}", url);

    let response = client
        .put(&url)
        .header("Authorization", config.basic_auth())
        .header("Content-Type", "application/json")
        .json(settings)
        .send()
        .await
        .context("Failed to propose settings")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Failed to propose settings: HTTP {} - {}", status, body);
    }

    response
        .json()
        .await
        .context("Failed to parse propose_settings response")
}

/// Get a value from a JSON object using dot notation path
fn get_value_at_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let parts: Vec<&str> = path.split('.').collect();
    let mut current = value;

    for part in parts {
        current = current.get(part)?;
    }

    Some(current)
}

/// Set a value in a JSON object using dot notation path
fn set_value_at_path(value: &mut Value, path: &str, new_value: Value) -> Result<()> {
    let parts: Vec<&str> = path.split('.').collect();

    if parts.is_empty() {
        anyhow::bail!("Empty path");
    }

    let mut current = value;

    // Navigate to the parent of the target
    for part in &parts[..parts.len() - 1] {
        current = current
            .get_mut(*part)
            .with_context(|| format!("Path component not found: {part}"))?;
    }

    // Set the final value
    let final_key = parts[parts.len() - 1];
    if let Some(obj) = current.as_object_mut() {
        obj.insert(final_key.to_string(), new_value);
        Ok(())
    } else {
        anyhow::bail!("Cannot set value: parent is not an object")
    }
}

/// Parse a value string as JSON or return as string
fn parse_value(value: &str) -> Value {
    // Try to parse as JSON first
    if let Ok(json) = serde_json::from_str(value) {
        return json;
    }

    // Check for common boolean values
    match value.to_lowercase().as_str() {
        "true" => return Value::Bool(true),
        "false" => return Value::Bool(false),
        _ => {}
    }

    // Check for numbers
    if let Ok(n) = value.parse::<i64>() {
        return Value::Number(n.into());
    }
    if let Ok(n) = value.parse::<f64>() {
        if let Some(num) = serde_json::Number::from_f64(n) {
            return Value::Number(num);
        }
    }

    // Fall back to string
    Value::String(value.to_string())
}

/// Format a JSON value for display
fn format_value(value: &Value) -> String {
    match value {
        Value::Null => "null".dimmed().to_string(),
        Value::Bool(b) => {
            if *b {
                "true".green().to_string()
            } else {
                "false".red().to_string()
            }
        }
        Value::Number(n) => n.to_string().cyan().to_string(),
        Value::String(s) => format!("\"{s}\"").yellow().to_string(),
        Value::Array(_) | Value::Object(_) => {
            serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
        }
    }
}

/// Print settings as a tree structure
fn print_settings_tree(value: &Value, indent: usize) {
    let prefix = "  ".repeat(indent);

    match value {
        Value::Object(obj) => {
            for (key, val) in obj {
                match val {
                    Value::Object(_) => {
                        println!("{}{}:", prefix, key.bold());
                        print_settings_tree(val, indent + 1);
                    }
                    Value::Array(arr) => {
                        println!("{}{}: [", prefix, key.bold());
                        for item in arr {
                            print_settings_tree(item, indent + 1);
                        }
                        println!("{prefix}]");
                    }
                    _ => {
                        println!("{}{}: {}", prefix, key, format_value(val));
                    }
                }
            }
        }
        _ => {
            println!("{}{}", prefix, format_value(value));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_value_at_path() {
        let settings = serde_json::json!({
            "kubernetes": {
                "version": "1.28.0",
                "enabled": true
            },
            "containerEngine": {
                "name": "containerd"
            }
        });

        assert_eq!(
            get_value_at_path(&settings, "kubernetes.version"),
            Some(&Value::String("1.28.0".to_string()))
        );
        assert_eq!(
            get_value_at_path(&settings, "kubernetes.enabled"),
            Some(&Value::Bool(true))
        );
        assert_eq!(
            get_value_at_path(&settings, "containerEngine.name"),
            Some(&Value::String("containerd".to_string()))
        );
        assert_eq!(get_value_at_path(&settings, "nonexistent"), None);
        assert_eq!(get_value_at_path(&settings, "kubernetes.nonexistent"), None);
    }

    #[test]
    fn test_set_value_at_path() {
        let mut settings = serde_json::json!({
            "kubernetes": {
                "version": "1.28.0",
                "enabled": true
            }
        });

        set_value_at_path(
            &mut settings,
            "kubernetes.version",
            Value::String("1.29.0".to_string()),
        )
        .unwrap();

        assert_eq!(
            settings["kubernetes"]["version"],
            Value::String("1.29.0".to_string())
        );
    }

    #[test]
    fn test_parse_value() {
        assert_eq!(parse_value("true"), Value::Bool(true));
        assert_eq!(parse_value("false"), Value::Bool(false));
        assert_eq!(parse_value("42"), Value::Number(42.into()));
        assert_eq!(parse_value("hello"), Value::String("hello".to_string()));
        assert_eq!(parse_value("[1, 2, 3]"), serde_json::json!([1, 2, 3]));
        assert_eq!(
            parse_value("{\"key\": \"value\"}"),
            serde_json::json!({"key": "value"})
        );
    }
}
