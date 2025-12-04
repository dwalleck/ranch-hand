//! Comprehensive diagnostic checks for Rancher Desktop.
//!
//! Runs multiple checks to verify Rancher Desktop health and identify issues.

use crate::cli::Cli;
use crate::client::http::{build_client, HttpClientConfig};
use crate::config::{ConfigError, RdEngineConfig};
use crate::paths::{arch_string, k3s_cache_dir};
use anyhow::Result;
use colored::Colorize;
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::net::TcpStream;
use std::time::Duration;
use tracing::{debug, info};

/// Status of a diagnostic check
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Ok,
    Warn,
    Fail,
    Skip,
}

impl CheckStatus {
    fn indicator(self) -> colored::ColoredString {
        match self {
            Self::Ok => "[OK]".green(),
            Self::Warn => "[WARN]".yellow(),
            Self::Fail => "[FAIL]".red(),
            Self::Skip => "[SKIP]".dimmed(),
        }
    }
}

/// Result of a single diagnostic check
#[derive(Debug, Clone, Serialize)]
pub struct CheckResult {
    /// Name of the check
    pub name: String,
    /// Status of the check
    pub status: CheckStatus,
    /// Human-readable message
    pub message: String,
    /// Additional details (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

impl CheckResult {
    fn ok(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: CheckStatus::Ok,
            message: message.into(),
            details: None,
        }
    }

    fn warn(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: CheckStatus::Warn,
            message: message.into(),
            details: None,
        }
    }

    fn fail(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: CheckStatus::Fail,
            message: message.into(),
            details: None,
        }
    }

    fn skip(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: CheckStatus::Skip,
            message: message.into(),
            details: None,
        }
    }

    fn with_details(mut self, details: impl Into<String>) -> Self {
        self.details = Some(details.into());
        self
    }
}

/// Output structure for the diagnose command
#[derive(Debug, Serialize)]
pub struct DiagnoseOutput {
    /// All check results grouped by category
    pub categories: HashMap<String, Vec<CheckResult>>,
    /// Overall health status
    pub healthy: bool,
    /// Count of each status type
    pub summary: DiagnoseSummary,
}

#[derive(Debug, Serialize)]
pub struct DiagnoseSummary {
    pub ok: usize,
    pub warn: usize,
    pub fail: usize,
    pub skip: usize,
}

/// Run comprehensive diagnostic checks
pub async fn run(cli: &Cli) -> Result<()> {
    info!("Running diagnostic checks");

    let show_progress = !cli.quiet && !cli.json;

    if show_progress {
        println!("{}", "Rancher Desktop Diagnostics".bold().cyan());
        println!("{}", "=".repeat(40));
        println!();
    }

    let mut categories: HashMap<String, Vec<CheckResult>> = HashMap::new();

    // 1. Application Status
    let application_checks = check_application_status(cli, show_progress).await;
    let rd_running = application_checks
        .iter()
        .any(|c| c.name == "Rancher Desktop" && c.status == CheckStatus::Ok);
    categories.insert("Application Status".to_string(), application_checks);

    // 2. API Connectivity (only if RD is running)
    let connectivity_checks = if rd_running {
        check_api_connectivity(cli, show_progress).await
    } else {
        if show_progress {
            print_category_header("API Connectivity");
            let skip = CheckResult::skip("API Check", "Skipped - Rancher Desktop not running");
            print_check_result(&skip);
            println!();
        }
        vec![CheckResult::skip(
            "API Check",
            "Skipped - Rancher Desktop not running",
        )]
    };
    categories.insert("API Connectivity".to_string(), connectivity_checks);

    // 3. Cache Status
    let cache_checks = check_cache_status(show_progress);
    categories.insert("Cache Status".to_string(), cache_checks);

    // 4. Network Connectivity
    let network_checks = check_network_connectivity(cli, show_progress).await;
    categories.insert("Network Connectivity".to_string(), network_checks);

    // 5. Platform-specific checks
    let platform_checks = check_platform_specific(show_progress);
    categories.insert("Platform".to_string(), platform_checks);

    // Calculate summary
    let (ok, warn, fail, skip) = categories.values().flatten().fold(
        (0, 0, 0, 0),
        |(ok, warn, fail, skip), check| match check.status {
            CheckStatus::Ok => (ok + 1, warn, fail, skip),
            CheckStatus::Warn => (ok, warn + 1, fail, skip),
            CheckStatus::Fail => (ok, warn, fail + 1, skip),
            CheckStatus::Skip => (ok, warn, fail, skip + 1),
        },
    );

    let healthy = fail == 0;

    if cli.json {
        let output = DiagnoseOutput {
            categories,
            healthy,
            summary: DiagnoseSummary {
                ok,
                warn,
                fail,
                skip,
            },
        };
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else if !cli.quiet {
        // Print summary
        println!("{}", "Summary".bold());
        println!("{}", "=".repeat(40));
        println!(
            "{} {} passed, {} {} warnings, {} {} failed, {} skipped",
            ok.to_string().green(),
            "checks".green(),
            warn.to_string().yellow(),
            "checks with".yellow(),
            fail.to_string().red(),
            "checks".red(),
            skip
        );
        println!();

        if healthy {
            println!("{}", "System appears healthy!".green().bold());
        } else {
            println!(
                "{}",
                "Issues detected - see above for details.".red().bold()
            );
        }
    }

    Ok(())
}

fn print_category_header(name: &str) {
    println!("{}", name.bold());
    println!("{}", "-".repeat(name.len()));
}

fn print_check_result(check: &CheckResult) {
    println!(
        "{} {}: {}",
        check.status.indicator(),
        check.name,
        check.message
    );
    if let Some(details) = &check.details {
        for line in details.lines() {
            println!("      {line}");
        }
    }
}

/// Check if Rancher Desktop is running and accessible
async fn check_application_status(cli: &Cli, show_progress: bool) -> Vec<CheckResult> {
    let mut results = Vec::new();

    if show_progress {
        print_category_header("Application Status");
    }

    // Check if rd-engine.json exists
    let config_result = RdEngineConfig::load();
    let rd_check = match &config_result {
        Ok(config) => {
            debug!(
                "Found rd-engine.json, API at {}:{}",
                config.host, config.port
            );
            CheckResult::ok(
                "Rancher Desktop",
                format!("Running (API on {}:{})", config.host, config.port),
            )
        }
        Err(ConfigError::NotFound { path }) => {
            CheckResult::fail("Rancher Desktop", "Not running or not installed")
                .with_details(format!("Config file not found: {path}"))
        }
        Err(e) => CheckResult::fail("Rancher Desktop", format!("Configuration error: {e}")),
    };

    if show_progress {
        print_check_result(&rd_check);
    }
    results.push(rd_check);

    // Check TCP connectivity to API port (only if config loaded)
    if let Ok(config) = config_result {
        let tcp_check = check_tcp_port(&config.host, config.port);
        if show_progress {
            print_check_result(&tcp_check);
        }
        results.push(tcp_check);

        // Try an HTTP request to the API
        let http_check = check_http_api(&config, cli).await;
        if show_progress {
            print_check_result(&http_check);
        }
        results.push(http_check);
    }

    if show_progress {
        println!();
    }

    results
}

fn check_tcp_port(host: &str, port: u16) -> CheckResult {
    let addr = format!("{host}:{port}");
    let socket_addr: std::net::SocketAddr = match addr.parse() {
        Ok(addr) => addr,
        Err(e) => {
            return CheckResult::fail("API Port", format!("Invalid address: {addr}"))
                .with_details(e.to_string())
        }
    };
    match TcpStream::connect_timeout(&socket_addr, Duration::from_secs(5)) {
        Ok(_) => CheckResult::ok("API Port", format!("Port {port} is accessible")),
        Err(e) => CheckResult::fail("API Port", format!("Cannot connect to port {port}"))
            .with_details(e.to_string()),
    }
}

async fn check_http_api(config: &RdEngineConfig, cli: &Cli) -> CheckResult {
    let client_config = HttpClientConfig::with_timeout(cli.insecure, cli.timeout);
    let client = match build_client(&client_config) {
        Ok(c) => c,
        Err(e) => {
            return CheckResult::fail("API Response", format!("Failed to build HTTP client: {e}"))
        }
    };

    let url = config.api_url("/v1/settings");
    // Note: timeout is already configured on the client via HttpClientConfig
    match client
        .get(&url)
        .header("Authorization", config.basic_auth())
        .send()
        .await
    {
        Ok(response) => {
            let status = response.status();
            if status.is_success() {
                CheckResult::ok("API Response", format!("API responding (HTTP {status})"))
            } else if status.as_u16() == 401 {
                CheckResult::warn("API Response", "Authentication required")
                    .with_details("Check rd-engine.json credentials")
            } else {
                CheckResult::warn("API Response", format!("Unexpected status: {status}"))
            }
        }
        Err(e) => {
            CheckResult::fail("API Response", "API request failed").with_details(e.to_string())
        }
    }
}

/// Check API connectivity (backend state, version, etc.)
async fn check_api_connectivity(cli: &Cli, show_progress: bool) -> Vec<CheckResult> {
    let mut results = Vec::new();

    if show_progress {
        print_category_header("API Connectivity");
    }

    let Ok(config) = RdEngineConfig::load() else {
        let skip = CheckResult::skip("Backend State", "Rancher Desktop not running");
        if show_progress {
            print_check_result(&skip);
            println!();
        }
        return vec![skip];
    };

    let client_config = HttpClientConfig::with_timeout(cli.insecure, cli.timeout);
    let client = match build_client(&client_config) {
        Ok(c) => c,
        Err(e) => {
            let fail = CheckResult::fail("HTTP Client", format!("Failed to build client: {e}"));
            if show_progress {
                print_check_result(&fail);
                println!();
            }
            return vec![fail];
        }
    };

    // Check backend state via /v1/backend_state
    let backend_check = check_backend_state(&client, &config).await;
    if show_progress {
        print_check_result(&backend_check);
    }
    results.push(backend_check);

    // Check version info
    let version_check = check_version_info(&client, &config).await;
    if show_progress {
        print_check_result(&version_check);
    }
    results.push(version_check);

    if show_progress {
        println!();
    }

    results
}

async fn check_backend_state(client: &reqwest::Client, config: &RdEngineConfig) -> CheckResult {
    let url = config.api_url("/v1/backend_state");
    // Note: timeout is already configured on the client via HttpClientConfig
    match client
        .get(&url)
        .header("Authorization", config.basic_auth())
        .send()
        .await
    {
        Ok(response) => {
            if response.status().is_success() {
                match response.text().await {
                    Ok(body) => {
                        // Parse the state - typically "STARTED", "STOPPED", etc.
                        let state = body.trim().trim_matches('"');
                        if state.to_uppercase() == "STARTED" {
                            CheckResult::ok("Backend State", "Backend is running")
                        } else {
                            CheckResult::warn("Backend State", format!("Backend state: {state}"))
                        }
                    }
                    Err(e) => CheckResult::warn("Backend State", "Could not read response")
                        .with_details(e.to_string()),
                }
            } else {
                CheckResult::warn("Backend State", format!("HTTP {}", response.status()))
            }
        }
        Err(e) => CheckResult::fail("Backend State", "Request failed").with_details(e.to_string()),
    }
}

async fn check_version_info(client: &reqwest::Client, config: &RdEngineConfig) -> CheckResult {
    let url = config.api_url("/v1/settings");
    // Note: timeout is already configured on the client via HttpClientConfig
    match client
        .get(&url)
        .header("Authorization", config.basic_auth())
        .send()
        .await
    {
        Ok(response) => {
            if response.status().is_success() {
                match response.json::<serde_json::Value>().await {
                    Ok(settings) => {
                        let k8s_version = settings
                            .get("kubernetes")
                            .and_then(|k| k.get("version"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown");

                        let container_engine = settings
                            .get("containerEngine")
                            .and_then(|c| c.get("name"))
                            .and_then(|n| n.as_str())
                            .unwrap_or("unknown");

                        CheckResult::ok(
                            "Configuration",
                            format!("k8s {k8s_version}, engine: {container_engine}"),
                        )
                    }
                    Err(e) => CheckResult::warn("Configuration", "Could not parse settings")
                        .with_details(e.to_string()),
                }
            } else {
                CheckResult::warn("Configuration", format!("HTTP {}", response.status()))
            }
        }
        Err(e) => CheckResult::fail("Configuration", "Request failed").with_details(e.to_string()),
    }
}

/// Check k3s cache status
fn check_cache_status(show_progress: bool) -> Vec<CheckResult> {
    let mut results = Vec::new();

    if show_progress {
        print_category_header("Cache Status");
    }

    let cache_check = match k3s_cache_dir() {
        Ok(cache_dir) => {
            if cache_dir.exists() {
                // Count versions in cache
                match fs::read_dir(&cache_dir) {
                    Ok(entries) => {
                        let versions: Vec<_> = entries
                            .filter_map(std::result::Result::ok)
                            .filter(|e| e.path().is_dir())
                            .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
                            .collect();

                        if versions.is_empty() {
                            CheckResult::warn("k3s Cache", "Cache directory exists but is empty")
                                .with_details(format!("Location: {}", cache_dir.display()))
                        } else {
                            let version_list: Vec<_> = versions
                                .iter()
                                .map(|v| v.file_name().to_string_lossy().to_string())
                                .collect();
                            CheckResult::ok(
                                "k3s Cache",
                                format!("{} version(s) cached", versions.len()),
                            )
                            .with_details(format!(
                                "Location: {}\nVersions: {}",
                                cache_dir.display(),
                                version_list.join(", ")
                            ))
                        }
                    }
                    Err(e) => CheckResult::warn("k3s Cache", "Could not read cache directory")
                        .with_details(e.to_string()),
                }
            } else {
                CheckResult::ok(
                    "k3s Cache",
                    "No cache directory (will be created on first use)",
                )
                .with_details(format!("Expected location: {}", cache_dir.display()))
            }
        }
        Err(e) => CheckResult::fail("k3s Cache", "Could not determine cache location")
            .with_details(e.to_string()),
    };

    if show_progress {
        print_check_result(&cache_check);
        println!();
    }
    results.push(cache_check);

    results
}

/// URL endpoints required by Rancher Desktop
/// See: https://docs.rancherdesktop.io/getting-started/installation#proxy-environments-important-url-patterns
const REQUIRED_ENDPOINTS: &[(&str, &str)] = &[
    (
        "K3s Releases API",
        "https://api.github.com/repos/k3s-io/k3s/releases",
    ),
    (
        "K3s Downloads",
        "https://github.com/k3s-io/k3s/releases/download",
    ),
    (
        "kubectl Releases",
        "https://storage.googleapis.com/kubernetes-release/release",
    ),
    (
        "Version Check",
        "https://desktop.version.rancher.io/v1/checkupgrade",
    ),
    ("Documentation", "https://docs.rancherdesktop.io"),
];

/// Check network connectivity to required domains
async fn check_network_connectivity(cli: &Cli, show_progress: bool) -> Vec<CheckResult> {
    if show_progress {
        print_category_header("Network Connectivity");
    }

    // Run HTTPS checks and DNS check concurrently for better performance
    let https_futures: Vec<_> = REQUIRED_ENDPOINTS
        .iter()
        .map(|(name, url)| check_https_connectivity(name, url, cli))
        .collect();

    let dns_future = check_dns_resolution("api.github.com");

    let (https_results, dns_check) =
        tokio::join!(futures_util::future::join_all(https_futures), dns_future);

    let mut results: Vec<CheckResult> = https_results;

    if show_progress {
        for result in &results {
            print_check_result(result);
        }
        print_check_result(&dns_check);
        println!();
    }

    results.push(dns_check);
    results
}

/// Timeout for network connectivity checks
const NETWORK_CHECK_TIMEOUT_SECS: u64 = 10;

async fn check_https_connectivity(name: &str, url: &str, cli: &Cli) -> CheckResult {
    let client_config = HttpClientConfig::with_timeout(cli.insecure, NETWORK_CHECK_TIMEOUT_SECS);
    let client = match build_client(&client_config) {
        Ok(c) => c,
        Err(e) => return CheckResult::fail(name, format!("Client error: {e}")),
    };

    // Note: timeout is already configured on the client via HttpClientConfig
    match client.head(url).send().await {
        Ok(response) => {
            let status = response.status();
            if status.is_success() || status.is_redirection() {
                CheckResult::ok(name, format!("OK (HTTP {status})"))
            } else {
                CheckResult::warn(name, format!("HTTP {status}")).with_details(url.to_string())
            }
        }
        Err(e) => {
            let error_str = e.to_string().to_lowercase();
            if error_str.contains("certificate")
                || error_str.contains("ssl")
                || error_str.contains("tls")
            {
                CheckResult::fail(name, "SSL/TLS error (possible proxy)").with_details(format!(
                    "{url}\n{e}\n\nRun 'rh certs check' for detailed certificate analysis"
                ))
            } else if e.is_timeout() {
                CheckResult::fail(name, "Connection timed out").with_details(url.to_string())
            } else if e.is_connect() {
                CheckResult::fail(name, "Connection failed")
                    .with_details(format!("{url}\n{e}"))
            } else {
                CheckResult::fail(name, "Request failed")
                    .with_details(format!("{url}\n{e}"))
            }
        }
    }
}

/// Timeout for DNS resolution checks
const DNS_RESOLUTION_TIMEOUT_SECS: u64 = 5;

async fn check_dns_resolution(domain: &str) -> CheckResult {
    use std::net::ToSocketAddrs;

    let domain = domain.to_string();
    let dns_future = tokio::task::spawn_blocking(move || {
        let addr = format!("{domain}:443");
        match addr.to_socket_addrs() {
            Ok(mut addrs) => {
                if let Some(addr) = addrs.next() {
                    CheckResult::ok("DNS Resolution", format!("{domain} → {}", addr.ip()))
                } else {
                    CheckResult::fail("DNS Resolution", format!("No addresses for {domain}"))
                }
            }
            Err(e) => CheckResult::fail("DNS Resolution", format!("Failed to resolve {domain}"))
                .with_details(e.to_string()),
        }
    });

    match tokio::time::timeout(Duration::from_secs(DNS_RESOLUTION_TIMEOUT_SECS), dns_future).await {
        Ok(result) => result
            .unwrap_or_else(|_| CheckResult::fail("DNS Resolution", "DNS check task panicked")),
        Err(_) => CheckResult::fail("DNS Resolution", "DNS resolution timed out"),
    }
}

/// Platform-specific checks
fn check_platform_specific(show_progress: bool) -> Vec<CheckResult> {
    let mut results = Vec::new();

    if show_progress {
        print_category_header("Platform");
    }

    // OS info
    let os_check = CheckResult::ok(
        "Operating System",
        format!("{} ({})", std::env::consts::OS, arch_string()),
    );
    if show_progress {
        print_check_result(&os_check);
    }
    results.push(os_check);

    // Platform-specific checks
    #[cfg(target_os = "linux")]
    {
        // Check if running in WSL
        let wsl_check = check_wsl();
        if show_progress {
            print_check_result(&wsl_check);
        }
        results.push(wsl_check);
    }

    #[cfg(target_os = "macos")]
    {
        // Check for Lima/QEMU
        let vm_check = check_macos_vm();
        if show_progress {
            print_check_result(&vm_check);
        }
        results.push(vm_check);
    }

    #[cfg(target_os = "windows")]
    {
        // Check WSL status
        let wsl_check = check_windows_wsl();
        if show_progress {
            print_check_result(&wsl_check);
        }
        results.push(wsl_check);
    }

    if show_progress {
        println!();
    }

    results
}

#[cfg(target_os = "linux")]
fn check_wsl() -> CheckResult {
    // Check for WSL by looking at /proc/version
    if let Ok(version) = fs::read_to_string("/proc/version") {
        if version.to_lowercase().contains("microsoft") || version.to_lowercase().contains("wsl") {
            return CheckResult::ok("WSL", "Running in Windows Subsystem for Linux");
        }
    }
    CheckResult::ok("Environment", "Native Linux")
}

#[cfg(target_os = "macos")]
fn check_macos_vm() -> CheckResult {
    // Check if Lima socket exists (common path for Rancher Desktop)
    let lima_socket = dirs::home_dir()
        .map(|h| h.join(".lima/0/sock/qemu.sock"))
        .filter(|p| p.exists());

    if lima_socket.is_some() {
        CheckResult::ok("VM Backend", "Lima/QEMU detected")
    } else {
        CheckResult::ok(
            "VM Backend",
            "Lima socket not found (may use different backend)",
        )
    }
}

#[cfg(target_os = "windows")]
fn check_windows_wsl() -> CheckResult {
    // Try to run wsl --status
    match std::process::Command::new("wsl").arg("--status").output() {
        Ok(output) => {
            if output.status.success() {
                CheckResult::ok("WSL", "WSL is available")
            } else {
                CheckResult::warn("WSL", "WSL returned non-zero status")
            }
        }
        Err(_) => CheckResult::warn("WSL", "Could not check WSL status"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_result_ok() {
        let result = CheckResult::ok("Test", "Everything is fine");
        assert_eq!(result.name, "Test");
        assert_eq!(result.status, CheckStatus::Ok);
        assert_eq!(result.message, "Everything is fine");
        assert!(result.details.is_none());
    }

    #[test]
    fn test_check_result_warn() {
        let result = CheckResult::warn("Test", "Minor issue");
        assert_eq!(result.status, CheckStatus::Warn);
    }

    #[test]
    fn test_check_result_fail() {
        let result = CheckResult::fail("Test", "Critical error");
        assert_eq!(result.status, CheckStatus::Fail);
    }

    #[test]
    fn test_check_result_skip() {
        let result = CheckResult::skip("Test", "Skipped check");
        assert_eq!(result.status, CheckStatus::Skip);
    }

    #[test]
    fn test_check_result_with_details() {
        let result = CheckResult::ok("Test", "Message").with_details("Extra info");
        assert_eq!(result.details, Some("Extra info".to_string()));
    }

    #[test]
    fn test_check_status_equality() {
        assert_eq!(CheckStatus::Ok, CheckStatus::Ok);
        assert_ne!(CheckStatus::Ok, CheckStatus::Fail);
    }

    #[test]
    fn test_diagnose_summary_counts() {
        let summary = DiagnoseSummary {
            ok: 5,
            warn: 2,
            fail: 1,
            skip: 0,
        };
        assert_eq!(summary.ok, 5);
        assert_eq!(summary.warn, 2);
        assert_eq!(summary.fail, 1);
        assert_eq!(summary.skip, 0);
    }
}
