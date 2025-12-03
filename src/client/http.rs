// This module provides HTTP client infrastructure that will be used by command implementations.
// Allow dead_code during infrastructure phase - will be removed when commands are implemented.
#![allow(dead_code)]

//! HTTP client with SSL certificate bypass support.
//!
//! # Why Certificate Bypass?
//!
//! This module intentionally provides the ability to bypass SSL certificate validation.
//! This is a core feature of ranch-hand, not a security oversight.
//!
//! Many corporate environments use SSL inspection proxies (e.g., Zscaler, iboss, `BlueCoat`)
//! that intercept HTTPS traffic by presenting their own certificates. This causes
//! certificate validation failures when downloading k3s releases from GitHub or
//! connecting to other external services.
//!
//! Users in these environments have two options:
//! 1. Request IT to whitelist specific domains (often slow or impossible)
//! 2. Use the `--insecure` flag to bypass validation (with user consent)
//!
//! The tool provides interactive prompts to ensure users understand the security
//! implications before proceeding with certificate bypass.

use anyhow::{Context, Result};
use dialoguer::Confirm;
use reqwest::Client;
use std::io::IsTerminal;
use thiserror::Error;
use tracing::warn;

#[derive(Error, Debug)]
pub enum HttpClientError {
    #[error("SSL certificate validation failed for {domain}: {reason}")]
    CertificateError { domain: String, reason: String },
    #[error("Connection refused - is Rancher Desktop running?")]
    ConnectionRefused,
    #[error("Request failed: {0}")]
    RequestFailed(String),
}

/// Default timeout for API requests (30 seconds)
pub const DEFAULT_API_TIMEOUT_SECS: u64 = 30;

/// Default timeout for file downloads (10 minutes - k3s images can be large)
pub const DEFAULT_DOWNLOAD_TIMEOUT_SECS: u64 = 600;

/// Configuration for the HTTP client
#[derive(Clone, Debug)]
pub struct HttpClientConfig {
    /// Accept invalid SSL certificates
    pub insecure: bool,
    /// Enable interactive prompts for certificate errors
    pub interactive: bool,
    /// Request timeout in seconds
    pub timeout_secs: u64,
}

impl Default for HttpClientConfig {
    fn default() -> Self {
        Self {
            insecure: false,
            interactive: true,
            timeout_secs: DEFAULT_API_TIMEOUT_SECS,
        }
    }
}

impl HttpClientConfig {
    /// Create a new config for API requests
    pub fn new(insecure: bool) -> Self {
        Self {
            insecure,
            interactive: !insecure, // Don't prompt if already insecure
            timeout_secs: DEFAULT_API_TIMEOUT_SECS,
        }
    }

    /// Create a new config with custom timeout
    pub fn with_timeout(insecure: bool, timeout_secs: u64) -> Self {
        Self {
            insecure,
            interactive: !insecure,
            timeout_secs,
        }
    }

    /// Create a config suitable for large file downloads
    pub fn for_downloads(insecure: bool) -> Self {
        Self {
            insecure,
            interactive: !insecure,
            timeout_secs: DEFAULT_DOWNLOAD_TIMEOUT_SECS,
        }
    }

    /// Create a config for downloads with custom timeout
    pub fn for_downloads_with_timeout(insecure: bool, timeout_secs: u64) -> Self {
        Self {
            insecure,
            interactive: !insecure,
            timeout_secs,
        }
    }
}

/// Build an HTTP client with optional SSL certificate bypass.
///
/// # Security Note
///
/// When `config.insecure` is true, this client will accept ANY certificate,
/// including self-signed, expired, or mismatched certificates. This is
/// intentional for corporate proxy environments - see module documentation.
pub fn build_client(config: &HttpClientConfig) -> Result<Client> {
    if config.insecure {
        warn!("Building HTTP client with certificate validation DISABLED");
    }

    let builder = Client::builder()
        .danger_accept_invalid_certs(config.insecure)
        .timeout(std::time::Duration::from_secs(config.timeout_secs));

    builder.build().context("Failed to build HTTP client")
}

/// Build an insecure HTTP client (bypasses all certificate validation).
///
/// # Security Note
///
/// This function is used when the user explicitly consents to bypass
/// certificate validation, either via `--insecure` flag or interactive prompt.
/// See module documentation for why this feature exists.
pub fn build_insecure_client() -> Result<Client> {
    warn!("Certificate validation bypassed by user request");
    build_client(&HttpClientConfig::new(true))
}

/// Attempt a request, handling certificate errors with optional interactive prompt
pub async fn request_with_cert_handling(
    url: &str,
    config: &HttpClientConfig,
) -> Result<reqwest::Response> {
    // First try with the configured client
    let client = build_client(config)?;

    match client.get(url).send().await {
        Ok(response) => Ok(response),
        Err(e) => {
            // Check if this is a certificate error
            if is_certificate_error(&e) {
                handle_certificate_error(url, &e, config).await
            } else if e.is_connect() {
                Err(HttpClientError::ConnectionRefused.into())
            } else {
                Err(HttpClientError::RequestFailed(e.to_string()).into())
            }
        }
    }
}

/// Check if an error is related to SSL certificates
fn is_certificate_error(error: &reqwest::Error) -> bool {
    let error_str = error.to_string().to_lowercase();
    error_str.contains("certificate")
        || error_str.contains("ssl")
        || error_str.contains("tls")
        || error_str.contains("self signed")
        || error_str.contains("unable to get local issuer")
}

/// Handle certificate errors with optional interactive prompt
async fn handle_certificate_error(
    url: &str,
    error: &reqwest::Error,
    config: &HttpClientConfig,
) -> Result<reqwest::Response> {
    let domain = extract_domain(url);
    let error_reason = extract_cert_error_reason(error);

    // If already in insecure mode, this shouldn't happen, but propagate the error
    if config.insecure {
        return Err(HttpClientError::CertificateError {
            domain,
            reason: error_reason,
        }
        .into());
    }

    // If interactive mode is enabled, prompt the user
    if config.interactive && std::io::stdin().is_terminal() {
        eprintln!();
        eprintln!("Certificate validation failed for {domain}");
        eprintln!("Reason: {error_reason}");
        eprintln!();

        if detect_corporate_proxy(&error_reason) {
            eprintln!("This appears to be a corporate SSL inspection proxy.");
            eprintln!();
        }

        let proceed = Confirm::new()
            .with_prompt("Do you want to proceed anyway? (insecure)")
            .default(false)
            .interact()
            .unwrap_or_else(|e| {
                // Print to stderr so users understand why the operation was denied,
                // in addition to logging for diagnostics
                eprintln!("Failed to get user confirmation: {e}");
                eprintln!("Defaulting to deny for security.");
                warn!("Failed to get user confirmation: {e}, defaulting to deny");
                false
            });

        if proceed {
            let insecure_client = build_insecure_client()?;
            return insecure_client
                .get(url)
                .send()
                .await
                .context("Request failed even with certificate bypass");
        }
    }

    Err(HttpClientError::CertificateError {
        domain,
        reason: error_reason,
    }
    .into())
}

/// Extract domain from URL
fn extract_domain(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(u) => u.host_str().unwrap_or("unknown").to_string(),
        Err(e) => {
            warn!("Failed to parse URL '{url}': {e}");
            "unknown".to_string()
        }
    }
}

/// Extract a human-readable reason from certificate errors
fn extract_cert_error_reason(error: &reqwest::Error) -> String {
    let error_str = error.to_string();

    if error_str.contains("self signed") || error_str.contains("SELF_SIGNED") {
        return "Self-signed certificate in chain".to_string();
    }
    if error_str.contains("unable to get local issuer") {
        return "Unable to verify certificate chain".to_string();
    }
    if error_str.contains("certificate has expired") {
        return "Certificate has expired".to_string();
    }
    if error_str.contains("hostname mismatch") {
        return "Certificate hostname mismatch".to_string();
    }

    // Fall back to the raw error
    format!("Certificate error: {error_str}")
}

/// Detect if the certificate error is likely from a corporate proxy
fn detect_corporate_proxy(reason: &str) -> bool {
    let lower = reason.to_lowercase();
    lower.contains("self signed")
        || lower.contains("self_signed")
        || lower.contains("unable to get local issuer")
}

/// Known corporate proxy certificate issuers
pub const KNOWN_PROXY_ISSUERS: &[&str] = &[
    "iboss",
    "zscaler",
    "bluecoat",
    "forcepoint",
    "symantec",
    "mcafee",
    "cisco",
    "palo alto",
    "fortinet",
    "websense",
    "netskope",
];

/// Check if a certificate issuer looks like a corporate proxy
pub fn is_proxy_issuer(issuer: &str) -> bool {
    let lower = issuer.to_lowercase();
    KNOWN_PROXY_ISSUERS
        .iter()
        .any(|proxy| lower.contains(proxy))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_domain() {
        assert_eq!(extract_domain("https://github.com/foo"), "github.com");
        assert_eq!(
            extract_domain("https://api.github.com/repos"),
            "api.github.com"
        );
        assert_eq!(extract_domain("invalid"), "unknown");
    }

    #[test]
    fn test_detect_corporate_proxy() {
        assert!(detect_corporate_proxy("self signed certificate in chain"));
        assert!(detect_corporate_proxy("SELF_SIGNED_CERT_IN_CHAIN"));
        assert!(detect_corporate_proxy(
            "unable to get local issuer certificate"
        ));
        assert!(!detect_corporate_proxy("connection refused"));
    }

    #[test]
    fn test_is_proxy_issuer() {
        assert!(is_proxy_issuer("iboss Network Security"));
        assert!(is_proxy_issuer("Zscaler Root CA"));
        assert!(is_proxy_issuer("BlueCoat ProxySG"));
        assert!(!is_proxy_issuer("DigiCert Global Root CA"));
        assert!(!is_proxy_issuer("Let's Encrypt"));
    }

    #[test]
    fn test_client_config_default() {
        let config = HttpClientConfig::default();
        assert!(!config.insecure);
        assert!(config.interactive);
        assert_eq!(config.timeout_secs, DEFAULT_API_TIMEOUT_SECS);
    }

    #[test]
    fn test_client_config_insecure() {
        let config = HttpClientConfig::new(true);
        assert!(config.insecure);
        assert!(!config.interactive); // Should disable prompts when insecure
        assert_eq!(config.timeout_secs, DEFAULT_API_TIMEOUT_SECS);
    }

    #[test]
    fn test_client_config_for_downloads() {
        let config = HttpClientConfig::for_downloads(false);
        assert!(!config.insecure);
        assert!(config.interactive);
        assert_eq!(config.timeout_secs, DEFAULT_DOWNLOAD_TIMEOUT_SECS);
    }
}
