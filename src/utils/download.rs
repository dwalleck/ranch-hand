//! File download utilities with progress display.
//!
//! Note: Some functions here are utilities for future use and not currently called.

#![allow(dead_code)]

use anyhow::{Context, Result};
use futures_util::StreamExt;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use reqwest::Client;
use std::path::Path;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;

/// Style for download progress bars
fn download_progress_style() -> ProgressStyle {
    ProgressStyle::default_bar()
        .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}, {eta})")
        .expect("Invalid progress template")
        .progress_chars("#>-")
}

/// Style for spinner when size is unknown
fn spinner_style() -> ProgressStyle {
    ProgressStyle::default_spinner()
        .template("{spinner:.green} [{elapsed_precise}] {bytes} ({bytes_per_sec})")
        .expect("Invalid spinner template")
}

/// Download a file with progress display.
///
/// Returns the number of bytes downloaded.
pub async fn download_file(
    client: &Client,
    url: &str,
    output_path: &Path,
    progress: Option<&ProgressBar>,
) -> Result<u64> {
    let response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("Failed to request {url}"))?
        .error_for_status()
        .with_context(|| format!("HTTP error for {url}"))?;

    let total_size = response.content_length();

    // Set up progress bar
    if let Some(pb) = progress {
        if let Some(size) = total_size {
            pb.set_length(size);
            pb.set_style(download_progress_style());
        } else {
            pb.set_style(spinner_style());
        }
    }

    // Create parent directories if needed
    if let Some(parent) = output_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("Failed to create directory {}", parent.display()))?;
    }

    // Download with streaming
    let mut file = File::create(output_path)
        .await
        .with_context(|| format!("Failed to create {}", output_path.display()))?;

    let mut stream = response.bytes_stream();
    let mut downloaded: u64 = 0;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.with_context(|| format!("Error downloading {url}"))?;
        file.write_all(&chunk)
            .await
            .with_context(|| format!("Failed to write to {}", output_path.display()))?;

        downloaded += chunk.len() as u64;
        if let Some(pb) = progress {
            pb.set_position(downloaded);
        }
    }

    file.flush()
        .await
        .with_context(|| format!("Failed to flush {}", output_path.display()))?;

    if let Some(pb) = progress {
        pb.finish_with_message("done");
    }

    Ok(downloaded)
}

/// Download a file with a new progress bar showing the filename.
pub async fn download_file_with_progress(
    client: &Client,
    url: &str,
    output_path: &Path,
    display_name: &str,
) -> Result<u64> {
    let pb = ProgressBar::new(0);
    pb.set_message(display_name.to_string());
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{msg}: {spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}, {eta})")
            .expect("Invalid progress template")
            .progress_chars("#>-"),
    );

    download_file(client, url, output_path, Some(&pb)).await
}

/// Context for managing multiple concurrent downloads.
pub struct DownloadManager {
    multi_progress: MultiProgress,
}

impl DownloadManager {
    pub fn new() -> Self {
        Self {
            multi_progress: MultiProgress::new(),
        }
    }

    /// Create a new progress bar for a download.
    pub fn add_download(&self, display_name: &str) -> ProgressBar {
        let pb = self.multi_progress.add(ProgressBar::new(0));
        pb.set_message(display_name.to_string());
        pb.set_style(
            ProgressStyle::default_bar()
                .template(
                    "{msg}: {spinner:.green} [{bar:30.cyan/blue}] {bytes}/{total_bytes} ({eta})",
                )
                .expect("Invalid progress template")
                .progress_chars("#>-"),
        );
        pb
    }

    /// Mark a progress bar as finished successfully.
    pub fn finish_success(pb: &ProgressBar, message: &str) {
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{msg}")
                .expect("Invalid progress template"),
        );
        pb.finish_with_message(format!("\u{2714} {message}")); // ✔
    }

    /// Mark a progress bar as failed.
    pub fn finish_error(pb: &ProgressBar, message: &str) {
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{msg}")
                .expect("Invalid progress template"),
        );
        pb.abandon_with_message(format!("\u{2718} {message}")); // ✘
    }
}

impl Default for DownloadManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_download_manager_creation() {
        let manager = DownloadManager::new();
        let pb = manager.add_download("test-file.tar.gz");
        assert_eq!(pb.message(), "test-file.tar.gz");
    }
}
