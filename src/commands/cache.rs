//! Cache management commands for k3s files.

use crate::cli::Cli;
use crate::client::http::{build_client, HttpClientConfig};
use crate::paths::{arch_string, k3s_binary_name, k3s_cache_dir, k3s_version_cache_dir};
use crate::utils::checksum::{parse_checksum_file, verify_file_from_checksums, ChecksumError};
use crate::utils::download::{
    check_existing_file, cleanup_partial_download, stream_to_file, DownloadManager,
};
use anyhow::{anyhow, Context, Result};
use colored::Colorize;
use dialoguer::FuzzySelect;
use futures_util::future::join_all;
use indicatif::{ProgressBar, ProgressStyle};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

/// k3s release base URL
const K3S_RELEASES_URL: &str = "https://github.com/k3s-io/k3s/releases/download";

/// k3s releases API URL
const K3S_RELEASES_API_URL: &str = "https://api.github.com/repos/k3s-io/k3s/releases";

/// Maximum number of versions to fetch from GitHub API
const MAX_VERSIONS_TO_FETCH: usize = 50;

/// GitHub release response structure
#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    prerelease: bool,
    draft: bool,
}

/// Files to download for cache populate
fn get_download_files(arch: &str) -> Vec<(&'static str, String)> {
    vec![
        ("binary", k3s_binary_name().to_string()),
        ("images", format!("k3s-airgap-images-{arch}.tar.zst")),
        ("checksums", format!("sha256sum-{arch}.txt")),
    ]
}

/// Represents a cached k3s file with its verification status.
#[derive(Debug, Clone, Serialize)]
pub struct CachedFile {
    /// Filename (e.g., "k3s", "k3s-airgap-images-amd64.tar.zst")
    pub name: String,
    /// File size in bytes
    pub size: u64,
    /// Checksum verification result: `Some(true)` = verified, `Some(false)` = mismatch, `None` = not checked
    pub verified: Option<bool>,
}

/// Represents a cached k3s version directory with its files.
#[derive(Debug, Clone, Serialize)]
pub struct CachedVersion {
    /// Version string (e.g., "v1.28.3+k3s1")
    pub version: String,
    /// Full path to the version's cache directory
    pub path: PathBuf,
    /// List of cached files in this version
    pub files: Vec<CachedFile>,
    /// Whether all expected files are present
    pub complete: bool,
}

/// Output structure for the cache list command.
#[derive(Debug, Serialize)]
pub struct CacheListOutput {
    /// Path to the k3s cache directory
    pub cache_dir: PathBuf,
    /// List of cached versions
    pub versions: Vec<CachedVersion>,
    /// Total size of all cached files in bytes
    pub total_size: u64,
}

/// List cached k3s versions
#[allow(clippy::unused_async)] // Async required by command dispatch
pub async fn list(cli: &Cli) -> Result<()> {
    let cache_dir = k3s_cache_dir()?;

    if !cache_dir.exists() {
        return print_empty_cache(cli, &cache_dir);
    }

    let (versions, total_size) = scan_cache_versions(&cache_dir)?;

    if cli.json {
        let output = CacheListOutput {
            cache_dir,
            versions,
            total_size,
        };
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        print_cache_list(&cache_dir, &versions, total_size);
    }

    Ok(())
}

fn print_empty_cache(cli: &Cli, cache_dir: &Path) -> Result<()> {
    if cli.json {
        let output = CacheListOutput {
            cache_dir: cache_dir.to_path_buf(),
            versions: vec![],
            total_size: 0,
        };
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!("{}", "No k3s cache found.".yellow());
        println!("Cache directory: {}", cache_dir.display());
        println!();
        println!(
            "Use {} to download k3s files.",
            "rh cache populate <version>".cyan()
        );
    }
    Ok(())
}

fn scan_cache_versions(cache_dir: &Path) -> Result<(Vec<CachedVersion>, u64)> {
    let mut versions = Vec::new();
    let mut total_size: u64 = 0;

    let entries = fs::read_dir(cache_dir)
        .with_context(|| format!("Failed to read cache directory: {}", cache_dir.display()))?;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();

        if !path.is_dir() {
            continue;
        }

        #[allow(clippy::single_match_else)] // match is clearer when both branches have logic
        let version_name = match path.file_name() {
            Some(name) => name.to_string_lossy().to_string(),
            None => {
                warn!("Invalid cache directory path: {}", path.display());
                String::from("unknown")
            }
        };

        if version_name.starts_with('.') {
            continue;
        }

        let (files, version_size, complete) = scan_version_files(&path)?;
        total_size = total_size.saturating_add(version_size);

        versions.push(CachedVersion {
            version: version_name,
            path,
            files,
            complete,
        });
    }

    versions.sort_by(|a, b| b.version.cmp(&a.version));
    Ok((versions, total_size))
}

fn scan_version_files(path: &Path) -> Result<(Vec<CachedFile>, u64, bool)> {
    let mut files = Vec::new();
    let mut total_size: u64 = 0;
    let mut complete = true;

    // Read checksums file directly, avoiding TOCTOU race between exists() and read()
    let checksums_path = path.join(format!("sha256sum-{}.txt", arch_string()));
    let checksums = fs::read_to_string(&checksums_path)
        .ok()
        .and_then(|content| parse_checksum_file(&content).ok());

    let expected_files = get_download_files(arch_string());
    for (_, filename) in &expected_files {
        let file_path = path.join(filename);
        if file_path.exists() {
            let (cached_file, size) =
                create_cached_file_entry(&file_path, filename, checksums.as_ref())?;
            total_size = total_size.saturating_add(size);
            files.push(cached_file);
        } else {
            complete = false;
        }
    }

    // List any additional files
    if let Ok(dir_entries) = fs::read_dir(path) {
        for dir_entry in dir_entries.flatten() {
            let file_name = dir_entry.file_name().to_string_lossy().to_string();
            if !expected_files.iter().any(|(_, f)| f == &file_name) {
                if let Ok(metadata) = dir_entry.metadata() {
                    if metadata.is_file() {
                        total_size = total_size.saturating_add(metadata.len());
                        files.push(CachedFile {
                            name: file_name,
                            size: metadata.len(),
                            verified: None,
                        });
                    }
                }
            }
        }
    }

    Ok((files, total_size, complete))
}

fn create_cached_file_entry(
    file_path: &Path,
    filename: &str,
    checksums: Option<&HashMap<String, String>>,
) -> Result<(CachedFile, u64)> {
    let metadata = fs::metadata(file_path)?;
    let size = metadata.len();

    let verified = match checksums {
        Some(cs) => match verify_file_from_checksums(file_path, cs) {
            Ok(()) => Some(true),
            Err(e) => {
                // Only report false for actual checksum mismatches
                // For other errors (file not in checksums, I/O errors), return None
                if e.downcast_ref::<ChecksumError>()
                    .is_some_and(|ce| matches!(ce, ChecksumError::Mismatch { .. }))
                {
                    Some(false)
                } else {
                    debug!("Could not verify {}: {}", filename, e);
                    None
                }
            }
        },
        None => None,
    };

    Ok((
        CachedFile {
            name: filename.to_string(),
            size,
            verified,
        },
        size,
    ))
}

fn print_cache_list(cache_dir: &Path, versions: &[CachedVersion], total_size: u64) {
    println!("{}", "K3s Cache".bold());
    println!("Location: {}", cache_dir.display());
    println!();

    if versions.is_empty() {
        println!("{}", "No cached versions found.".yellow());
        println!();
        println!(
            "Use {} to download k3s files.",
            "rh cache populate <version>".cyan()
        );
        return;
    }

    for version in versions {
        let status = if version.complete {
            "\u{2714}".green() // ✔
        } else {
            "\u{26A0}".yellow() // ⚠
        };

        println!("{} {}", status, version.version.bold());

        for file in &version.files {
            let size_str = format_size(file.size);
            let verify_status = match file.verified {
                Some(true) => " (verified)".green().to_string(),
                Some(false) => " (checksum mismatch!)".red().to_string(),
                None => String::new(),
            };
            println!("    {} ({}){}", file.name, size_str, verify_status);
        }
        println!();
    }

    println!(
        "Total: {} versions, {}",
        versions.len(),
        format_size(total_size)
    );
}

#[allow(clippy::cast_precision_loss)] // Acceptable for human-readable size display
fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

/// Validate version string to prevent path traversal attacks.
fn validate_version(version: &str) -> Result<()> {
    if version.is_empty() {
        return Err(anyhow!("Version cannot be empty"));
    }

    // Check for path traversal attempts
    if version.contains('/') || version.contains('\\') || version.contains("..") {
        return Err(anyhow!(
            "Invalid version format: version cannot contain path separators or '..'"
        ));
    }

    // Check for null bytes which can cause path handling issues
    if version.contains('\0') {
        return Err(anyhow!(
            "Invalid version format: version cannot contain null bytes"
        ));
    }

    Ok(())
}

/// Fetch available k3s versions from GitHub API
async fn fetch_available_versions(cli: &Cli) -> Result<Vec<String>> {
    let url = format!("{K3S_RELEASES_API_URL}?per_page={MAX_VERSIONS_TO_FETCH}");
    debug!("Fetching k3s releases from: {}", url);

    let client = build_client(&HttpClientConfig::new(cli.insecure))?;
    let response = client
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "ranch-hand")
        .send()
        .await
        .context("Failed to fetch k3s releases from GitHub")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        let details = if body.is_empty() {
            "(no response body)".to_string()
        } else {
            body
        };
        return Err(anyhow!("GitHub API returned status {status}: {details}"));
    }

    let releases: Vec<GitHubRelease> = response
        .json()
        .await
        .context("Failed to parse GitHub releases response")?;

    let versions: Vec<String> = releases
        .into_iter()
        .filter(|r| !r.prerelease && !r.draft)
        .map(|r| r.tag_name)
        .collect();

    if versions.is_empty() {
        return Err(anyhow!("No stable k3s releases found"));
    }

    debug!("Found {} stable k3s versions", versions.len());
    Ok(versions)
}

/// Prompt user to select a k3s version interactively
fn select_version_interactive(versions: &[String]) -> Result<String> {
    if !std::io::stdin().is_terminal() {
        return Err(anyhow!(
            "No version specified and not running in interactive mode.\n\
             Please specify a version: rh cache populate <version>\n\
             Or run interactively to select from available versions."
        ));
    }

    println!("{}", "Select a k3s version to download:".cyan());
    println!();

    let selection = FuzzySelect::new()
        .with_prompt("Version (type to filter)")
        .items(versions)
        .default(0)
        .interact()
        .context("Failed to get version selection")?;

    Ok(versions[selection].clone())
}

/// Populate cache with k3s files for a specific version
pub async fn populate(cli: &Cli, version: Option<&str>, force: bool) -> Result<()> {
    // If no version provided, fetch available versions and let user select
    let version = if let Some(v) = version {
        v.to_string()
    } else {
        let spinner = if cli.quiet {
            None
        } else {
            let sp = ProgressBar::new_spinner();
            sp.set_style(
                ProgressStyle::default_spinner()
                    .template("{spinner:.cyan} {msg}")
                    .expect("valid spinner template"),
            );
            sp.set_message("Fetching available k3s versions...");
            sp.enable_steady_tick(std::time::Duration::from_millis(100));
            Some(sp)
        };

        let versions = fetch_available_versions(cli).await;

        if let Some(sp) = spinner {
            sp.finish_and_clear();
        }

        let versions = versions?;
        select_version_interactive(&versions)?
    };

    validate_version(&version)?;

    let arch = arch_string();
    let version_dir = k3s_version_cache_dir(&version)?;

    info!("Populating cache for k3s {version} ({arch})");
    debug!("Cache directory: {}", version_dir.display());

    print_populate_header(cli, &version, arch, &version_dir, force);

    fs::create_dir_all(&version_dir).with_context(|| {
        format!(
            "Failed to create cache directory: {}",
            version_dir.display()
        )
    })?;

    let checksums = download_checksums(cli, &version, arch, &version_dir).await?;
    download_remaining_files(cli, &version, arch, &version_dir, &checksums, force).await?;
    verify_and_print_success(cli, &version_dir, &checksums, force)?;

    Ok(())
}

fn print_populate_header(cli: &Cli, version: &str, arch: &str, version_dir: &Path, force: bool) {
    if !cli.quiet {
        println!("{}", "Rancher Desktop K3s Cache Setup".bold().cyan());
        println!();
        println!("Version: {}", version.yellow());
        println!("Architecture: {arch}");
        println!("Cache directory: {}", version_dir.display());
        if force {
            println!(
                "{} {}",
                "\u{26A0}".yellow(),
                "Force mode: checksum failures will be ignored".yellow()
            );
        }
        println!();
    }
}

async fn download_checksums(
    cli: &Cli,
    version: &str,
    arch: &str,
    version_dir: &Path,
) -> Result<HashMap<String, String>> {
    let checksums_filename = format!("sha256sum-{arch}.txt");
    let checksums_url = format!("{K3S_RELEASES_URL}/{version}/{checksums_filename}");
    let checksums_path = version_dir.join(&checksums_filename);

    if !cli.quiet {
        println!("{}", "Downloading files...".green());
    }

    let manager = DownloadManager::new();
    let pb = if cli.quiet {
        None
    } else {
        Some(manager.add_download(&checksums_filename))
    };

    download_with_progress(&checksums_url, &checksums_path, pb.as_ref(), cli).await?;

    if let Some(pb) = pb {
        DownloadManager::finish_success(&pb, &checksums_filename);
    }

    let checksums_content = fs::read_to_string(&checksums_path).with_context(|| {
        format!(
            "Failed to read checksums file: {}",
            checksums_path.display()
        )
    })?;
    parse_checksum_file(&checksums_content)
}

/// Result of a single download operation
struct DownloadResult {
    filename: String,
    progress_bar: Option<ProgressBar>,
    result: Result<PathBuf>,
    /// Verification result: `Some(Ok(()))` = verified, `Some(Err(_))` = failed, `None` = not verified
    verification: Option<Result<()>>,
}

async fn download_remaining_files(
    cli: &Cli,
    version: &str,
    arch: &str,
    version_dir: &Path,
    checksums: &HashMap<String, String>,
    force: bool,
) -> Result<()> {
    let files = get_download_files(arch);
    let manager = DownloadManager::new();

    // Build list of downloads to perform (excluding checksums)
    let downloads: Vec<_> = files
        .into_iter()
        .filter(|(file_type, _)| *file_type != "checksums")
        .map(|(file_type, filename)| {
            let pb = if cli.quiet {
                None
            } else {
                Some(manager.add_download(&filename))
            };
            (file_type, filename, pb)
        })
        .collect();

    // Create futures for all downloads (each will verify immediately after completing)
    let download_futures = downloads.into_iter().map(|(file_type, filename, pb)| {
        download_and_verify(
            file_type,
            filename,
            pb,
            version,
            version_dir,
            arch,
            cli,
            checksums,
        )
    });

    // Run all downloads in parallel - verification happens concurrently as each completes
    let results = join_all(download_futures).await;

    // Process and report results
    process_download_results(results, cli, force)
}

#[allow(clippy::too_many_arguments)] // All params needed for download + verification in one async task
async fn download_and_verify(
    file_type: &'static str,
    filename: String,
    progress_bar: Option<ProgressBar>,
    version: &str,
    version_dir: &Path,
    arch: &str,
    cli: &Cli,
    checksums: &HashMap<String, String>,
) -> DownloadResult {
    let result = if file_type == "images" {
        download_images_with_fallback(version, version_dir, arch, progress_bar.as_ref(), cli).await
    } else {
        let url = format!("{K3S_RELEASES_URL}/{version}/{filename}");
        let file_path = version_dir.join(&filename);
        download_with_progress(&url, &file_path, progress_bar.as_ref(), cli).await
    };

    // Verify immediately after download completes (concurrent with other downloads)
    let verification = result.as_ref().ok().map(|path| {
        #[allow(clippy::single_match_else)]
        let actual_filename = match path.file_name() {
            Some(name) => name.to_string_lossy(),
            None => {
                warn!("Downloaded file has invalid path: {}", path.display());
                std::borrow::Cow::Borrowed("unknown")
            }
        };
        verify_file_from_checksums(path, checksums)
            .with_context(|| format!("Checksum verification failed for {actual_filename}"))
    });

    DownloadResult {
        filename,
        progress_bar,
        result,
        verification,
    }
}

fn process_download_results(results: Vec<DownloadResult>, cli: &Cli, force: bool) -> Result<()> {
    let mut download_errors = Vec::new();
    let mut verification_errors = Vec::new();

    for download in results {
        match download.result {
            Ok(downloaded_path) => {
                #[allow(clippy::single_match_else)]
                let actual_filename = match downloaded_path.file_name() {
                    Some(name) => name.to_string_lossy(),
                    None => {
                        warn!(
                            "Downloaded file has invalid path: {}",
                            downloaded_path.display()
                        );
                        std::borrow::Cow::Borrowed("unknown")
                    }
                };

                // Report verification result (verification already happened concurrently)
                match &download.verification {
                    Some(Ok(())) => {
                        debug!("Checksum verified for {}", actual_filename);
                        if let Some(pb) = &download.progress_bar {
                            DownloadManager::finish_success(pb, actual_filename.as_ref());
                        }
                    }
                    Some(Err(e)) => {
                        if force {
                            // Force mode: warn but continue
                            warn!(
                                "Checksum verification failed for {}: {} (continuing due to --force)",
                                actual_filename, e
                            );
                            if !cli.quiet {
                                println!(
                                    "  {} {}",
                                    "\u{26A0}".yellow(),
                                    format!("Checksum verification failed (ignored): {e}").yellow()
                                );
                            }
                            if let Some(pb) = &download.progress_bar {
                                DownloadManager::finish_success(pb, actual_filename.as_ref());
                            }
                        } else {
                            // Normal mode: collect as error
                            warn!(
                                "Checksum verification failed for {}: {}",
                                actual_filename, e
                            );
                            if let Some(pb) = &download.progress_bar {
                                DownloadManager::finish_error(pb, actual_filename.as_ref());
                            }
                            verification_errors
                                .push(format!("{actual_filename}: checksum verification failed"));
                        }
                    }
                    None => {
                        // No checksums available for this file
                        if let Some(pb) = &download.progress_bar {
                            DownloadManager::finish_success(pb, actual_filename.as_ref());
                        }
                    }
                }
            }
            Err(e) => {
                if let Some(pb) = download.progress_bar {
                    DownloadManager::finish_error(&pb, &download.filename);
                }
                download_errors.push(format!("{}: {}", download.filename, e));
            }
        }
    }

    // Report all errors
    let mut all_errors = download_errors;
    all_errors.extend(verification_errors);

    if all_errors.is_empty() {
        Ok(())
    } else {
        Err(anyhow!(
            "Failed to download/verify {} file(s):\n  {}",
            all_errors.len(),
            all_errors.join("\n  ")
        ))
    }
}

fn verify_and_print_success(
    cli: &Cli,
    version_dir: &Path,
    checksums: &HashMap<String, String>,
    force: bool,
) -> Result<()> {
    let binary_path = version_dir.join(k3s_binary_name());
    if binary_path.exists() {
        match verify_file_from_checksums(&binary_path, checksums) {
            Ok(()) => {
                if !cli.quiet {
                    println!();
                    println!("{} Binary checksum verified", "\u{2714}".green());
                }
            }
            Err(e) => {
                if force {
                    warn!(
                        "Binary checksum verification failed: {} (continuing due to --force)",
                        e
                    );
                    if !cli.quiet {
                        println!();
                        println!(
                            "{} {}",
                            "\u{26A0}".yellow(),
                            format!("Binary checksum verification failed (ignored): {e}").yellow()
                        );
                    }
                } else {
                    return Err(anyhow!("Binary checksum verification failed: {e}"));
                }
            }
        }
    }

    if !cli.quiet {
        println!();
        println!("{}", "========================================".green());
        println!(
            "{}",
            "SUCCESS! K3s files cached successfully.".green().bold()
        );
        println!("{}", "========================================".green());
        println!();
        println!("{}", "Next steps:".yellow());
        println!("  1. Close Rancher Desktop if it's running");
        println!("  2. Start Rancher Desktop");
        println!("  3. It should now start without downloading k3s");
        println!();
        println!("Cache location: {}", version_dir.display());
    }

    Ok(())
}

async fn download_with_progress(
    url: &str,
    path: &Path,
    progress: Option<&ProgressBar>,
    cli: &Cli,
) -> Result<PathBuf> {
    debug!("Downloading {} to {}", url, path.display());

    if let Some(existing) = check_existing_file(path, progress) {
        return Ok(existing);
    }

    let response = crate::client::http::request_with_cert_handling(
        url,
        &HttpClientConfig::for_downloads_with_timeout(cli.insecure, cli.download_timeout),
    )
    .await?;

    let total_size = response.content_length();
    if let Some(pb) = progress {
        if let Some(size) = total_size {
            pb.set_length(size);
        }
    }

    // Stream to file, cleaning up partial file on error
    if let Err(e) = stream_to_file(response, path, progress).await {
        cleanup_partial_download(path);
        return Err(e);
    }

    Ok(path.to_path_buf())
}

async fn download_images_with_fallback(
    version: &str,
    version_dir: &Path,
    arch: &str,
    progress: Option<&ProgressBar>,
    cli: &Cli,
) -> Result<PathBuf> {
    let formats = [
        format!("k3s-airgap-images-{arch}.tar.zst"),
        format!("k3s-airgap-images-{arch}.tar.gz"),
        format!("k3s-airgap-images-{arch}.tar"),
    ];

    let mut last_error = None;

    for filename in &formats {
        let url = format!("{K3S_RELEASES_URL}/{version}/{filename}");
        let file_path = version_dir.join(filename);

        debug!("Trying to download images: {}", url);

        if let Some(existing) = check_existing_file(&file_path, progress) {
            return Ok(existing);
        }

        match download_with_progress(&url, &file_path, progress, cli).await {
            Ok(path) => {
                info!("Successfully downloaded images: {}", filename);
                return Ok(path);
            }
            Err(e) => {
                debug!("Failed to download {}: {}", filename, e);
                last_error = Some(e);
                cleanup_partial_download(&file_path);
            }
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow!("Failed to download airgap images")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_version_valid() {
        assert!(validate_version("v1.28.3+k3s1").is_ok());
        assert!(validate_version("v1.33.3+k3s1").is_ok());
        assert!(validate_version("1.28.3").is_ok());
    }

    #[test]
    fn test_validate_version_empty() {
        let result = validate_version("");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("cannot be empty"));
    }

    #[test]
    fn test_validate_version_path_traversal() {
        // Forward slash
        let result = validate_version("../../../etc/passwd");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("path separators"));

        // Backslash
        let result = validate_version("..\\..\\etc\\passwd");
        assert!(result.is_err());

        // Double dot without slashes
        let result = validate_version("v1.28..3");
        assert!(result.is_err());

        // Just a slash
        let result = validate_version("v1/28");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_version_null_byte() {
        let result = validate_version("v1.28.3\0");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("null bytes"));
    }
}
