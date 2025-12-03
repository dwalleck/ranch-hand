//! Download progress display utilities.

use anyhow::{Context, Result};
use futures_util::StreamExt;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::fs;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;
use tracing::{info, warn};

/// Context for managing multiple concurrent downloads with progress bars.
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

/// Check if a file already exists with non-zero size.
///
/// If the file exists, updates the progress bar to show completion and returns the path.
/// Returns None if the file doesn't exist or is empty.
pub fn check_existing_file(path: &Path, progress: Option<&ProgressBar>) -> Option<PathBuf> {
    if path.exists() {
        if let Ok(metadata) = fs::metadata(path) {
            if metadata.len() > 0 {
                info!("File already exists, skipping: {}", path.display());
                if let Some(pb) = progress {
                    pb.set_length(metadata.len());
                    pb.set_position(metadata.len());
                }
                return Some(path.to_path_buf());
            }
        }
    }
    None
}

/// Stream a response body to a file with progress tracking.
///
/// This function streams the response data to the file in chunks,
/// updating the progress bar as data is written.
pub async fn stream_to_file(
    response: reqwest::Response,
    path: &Path,
    progress: Option<&ProgressBar>,
) -> Result<()> {
    let mut file = tokio::fs::File::create(path)
        .await
        .with_context(|| format!("Failed to create file: {}", path.display()))?;

    let mut stream = response.bytes_stream();
    let mut downloaded: u64 = 0;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.with_context(|| format!("Error downloading to {}", path.display()))?;
        file.write_all(&chunk)
            .await
            .with_context(|| format!("Failed to write to {}", path.display()))?;

        downloaded += chunk.len() as u64;
        if let Some(pb) = progress {
            pb.set_position(downloaded);
        }
    }

    file.flush()
        .await
        .with_context(|| format!("Failed to flush {}", path.display()))?;

    Ok(())
}

/// Clean up a partial download file, logging any errors.
pub fn cleanup_partial_download(path: &Path) {
    if let Err(cleanup_err) = fs::remove_file(path) {
        warn!(
            "Failed to clean up partial download {}: {}",
            path.display(),
            cleanup_err
        );
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

    #[test]
    fn test_check_existing_file_not_found() {
        let result = check_existing_file(Path::new("/nonexistent/file"), None);
        assert!(result.is_none());
    }
}
