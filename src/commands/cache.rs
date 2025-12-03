//! Cache management commands for k3s files.

use crate::cli::Cli;
use crate::client::http::HttpClientConfig;
use crate::paths::{arch_string, k3s_binary_name, k3s_cache_dir, k3s_version_cache_dir};
use crate::utils::checksum::{parse_checksum_file, verify_file_from_checksums, ChecksumError};
use crate::utils::download::{
    check_existing_file, cleanup_partial_download, stream_to_file, DownloadManager,
};
use anyhow::{anyhow, Context, Result};
use colored::Colorize;
use futures_util::future::join_all;
use indicatif::ProgressBar;
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

/// k3s release base URL
const K3S_RELEASES_URL: &str = "https://github.com/k3s-io/k3s/releases/download";

/// Files to download for cache populate
fn get_download_files(arch: &str) -> Vec<(&'static str, String)> {
    vec![
        ("binary", k3s_binary_name().to_string()),
        ("images", format!("k3s-airgap-images-{arch}.tar.zst")),
        ("checksums", format!("sha256sum-{arch}.txt")),
    ]
}

/// Status of a cached file
#[derive(Debug, Clone, Serialize)]
pub struct CachedFile {
    pub name: String,
    pub size: u64,
    pub verified: Option<bool>,
}

/// Status of a cached version
#[derive(Debug, Clone, Serialize)]
pub struct CachedVersion {
    pub version: String,
    pub path: PathBuf,
    pub files: Vec<CachedFile>,
    pub complete: bool,
}

/// Cache list output
#[derive(Debug, Serialize)]
pub struct CacheListOutput {
    pub cache_dir: PathBuf,
    pub versions: Vec<CachedVersion>,
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

        let version_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        if version_name.starts_with('.') {
            continue;
        }

        let (files, version_size, complete) = scan_version_files(&path)?;
        total_size += version_size;

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

    let checksums_path = path.join(format!("sha256sum-{}.txt", arch_string()));
    let checksums = if checksums_path.exists() {
        fs::read_to_string(&checksums_path)
            .ok()
            .and_then(|content| parse_checksum_file(&content).ok())
    } else {
        None
    };

    let expected_files = get_download_files(arch_string());
    for (_, filename) in &expected_files {
        let file_path = path.join(filename);
        if file_path.exists() {
            let (cached_file, size) =
                create_cached_file_entry(&file_path, filename, checksums.as_ref())?;
            total_size += size;
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
                        total_size += metadata.len();
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

/// Populate cache with k3s files for a specific version
pub async fn populate(cli: &Cli, version: &str) -> Result<()> {
    let arch = arch_string();
    let version_dir = k3s_version_cache_dir(version)?;

    info!("Populating cache for k3s {version} ({arch})");
    debug!("Cache directory: {}", version_dir.display());

    print_populate_header(cli, version, arch, &version_dir);

    fs::create_dir_all(&version_dir).with_context(|| {
        format!(
            "Failed to create cache directory: {}",
            version_dir.display()
        )
    })?;

    let checksums = download_checksums(cli, version, arch, &version_dir).await?;
    download_remaining_files(cli, version, arch, &version_dir, &checksums).await?;
    verify_and_print_success(cli, &version_dir, &checksums)?;

    Ok(())
}

fn print_populate_header(cli: &Cli, version: &str, arch: &str, version_dir: &Path) {
    if !cli.quiet {
        println!("{}", "Rancher Desktop K3s Cache Setup".bold().cyan());
        println!();
        println!("Version: {}", version.yellow());
        println!("Architecture: {arch}");
        println!("Cache directory: {}", version_dir.display());
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
}

async fn download_remaining_files(
    cli: &Cli,
    version: &str,
    arch: &str,
    version_dir: &Path,
    checksums: &HashMap<String, String>,
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

    // Create futures for all downloads
    let download_futures = downloads.into_iter().map(|(file_type, filename, pb)| {
        download_single_file(file_type, filename, pb, version, version_dir, arch, cli)
    });

    // Run all downloads in parallel
    let results = join_all(download_futures).await;

    // Process results and collect errors
    process_download_results(results, checksums, cli)
}

async fn download_single_file(
    file_type: &'static str,
    filename: String,
    progress_bar: Option<ProgressBar>,
    version: &str,
    version_dir: &Path,
    arch: &str,
    cli: &Cli,
) -> DownloadResult {
    let result = if file_type == "images" {
        download_images_with_fallback(version, version_dir, arch, progress_bar.as_ref(), cli).await
    } else {
        let url = format!("{K3S_RELEASES_URL}/{version}/{filename}");
        let file_path = version_dir.join(&filename);
        download_with_progress(&url, &file_path, progress_bar.as_ref(), cli).await
    };

    DownloadResult {
        filename,
        progress_bar,
        result,
    }
}

fn process_download_results(
    results: Vec<DownloadResult>,
    checksums: &HashMap<String, String>,
    cli: &Cli,
) -> Result<()> {
    let mut errors = Vec::new();

    for download in results {
        match download.result {
            Ok(downloaded_path) => {
                let actual_filename = downloaded_path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy();

                if let Some(pb) = &download.progress_bar {
                    DownloadManager::finish_success(pb, actual_filename.as_ref());
                }

                // Verify all downloaded files against checksums
                verify_downloaded_file(cli, &downloaded_path, actual_filename.as_ref(), checksums);
            }
            Err(e) => {
                if let Some(pb) = download.progress_bar {
                    DownloadManager::finish_error(&pb, &download.filename);
                }
                errors.push(format!("{}: {}", download.filename, e));
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(anyhow!(
            "Failed to download {} file(s):\n  {}",
            errors.len(),
            errors.join("\n  ")
        ))
    }
}

fn verify_downloaded_file(
    cli: &Cli,
    path: &Path,
    filename: &str,
    checksums: &HashMap<String, String>,
) {
    if let Err(e) = verify_file_from_checksums(path, checksums) {
        warn!("Checksum verification failed for {}: {}", filename, e);
        if !cli.quiet {
            println!(
                "  {} {}",
                "\u{26A0}".yellow(),
                format!("Checksum verification failed: {e}").yellow()
            );
        }
    } else {
        debug!("Checksum verified for {}", filename);
    }
}

fn verify_and_print_success(
    cli: &Cli,
    version_dir: &Path,
    checksums: &HashMap<String, String>,
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
                return Err(anyhow!("Binary checksum verification failed: {e}"));
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
