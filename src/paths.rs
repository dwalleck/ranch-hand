// This module provides path resolution infrastructure that will be used by command implementations.
// Allow dead_code during infrastructure phase - will be removed when commands are implemented.
#![allow(dead_code)]

use std::path::PathBuf;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum PathError {
    #[error("Could not determine cache directory for this platform")]
    NoCacheDir,
    #[error("Could not determine data directory for this platform")]
    NoDataDir,
}

/// Returns the base cache directory for Rancher Desktop k3s files.
///
/// Platform-specific paths:
/// - Windows: %LOCALAPPDATA%\rancher-desktop\cache\k3s
/// - macOS: ~/Library/Caches/rancher-desktop/k3s
/// - Linux: ~/.cache/rancher-desktop/k3s
pub fn k3s_cache_dir() -> Result<PathBuf, PathError> {
    #[cfg(target_os = "macos")]
    {
        dirs::home_dir()
            .map(|p| p.join("Library/Caches/rancher-desktop/k3s"))
            .ok_or(PathError::NoCacheDir)
    }

    #[cfg(target_os = "windows")]
    {
        dirs::data_local_dir()
            .map(|p| p.join("rancher-desktop").join("cache").join("k3s"))
            .ok_or(PathError::NoCacheDir)
    }

    #[cfg(target_os = "linux")]
    {
        dirs::cache_dir()
            .map(|p| p.join("rancher-desktop/k3s"))
            .ok_or(PathError::NoCacheDir)
    }
}

/// Returns the cache directory for a specific k3s version.
pub fn k3s_version_cache_dir(version: &str) -> Result<PathBuf, PathError> {
    Ok(k3s_cache_dir()?.join(version))
}

/// Returns the path to rd-engine.json containing API credentials.
///
/// Platform-specific paths:
/// - Windows: %LOCALAPPDATA%\rancher-desktop\rd-engine.json
/// - macOS: ~/Library/Application Support/rancher-desktop/rd-engine.json
/// - Linux: ~/.local/share/rancher-desktop/rd-engine.json
pub fn rd_engine_json_path() -> Result<PathBuf, PathError> {
    #[cfg(target_os = "macos")]
    {
        dirs::home_dir()
            .map(|p| p.join("Library/Application Support/rancher-desktop/rd-engine.json"))
            .ok_or(PathError::NoDataDir)
    }

    #[cfg(target_os = "windows")]
    {
        dirs::data_local_dir()
            .map(|p| p.join("rancher-desktop").join("rd-engine.json"))
            .ok_or(PathError::NoDataDir)
    }

    #[cfg(target_os = "linux")]
    {
        dirs::data_local_dir()
            .map(|p| p.join("rancher-desktop/rd-engine.json"))
            .ok_or(PathError::NoDataDir)
    }
}

/// Returns the Rancher Desktop data directory.
///
/// Platform-specific paths:
/// - Windows: %LOCALAPPDATA%\rancher-desktop
/// - macOS: ~/Library/Application Support/rancher-desktop
/// - Linux: ~/.local/share/rancher-desktop
pub fn rancher_desktop_data_dir() -> Result<PathBuf, PathError> {
    #[cfg(target_os = "macos")]
    {
        dirs::home_dir()
            .map(|p| p.join("Library/Application Support/rancher-desktop"))
            .ok_or(PathError::NoDataDir)
    }

    #[cfg(target_os = "windows")]
    {
        dirs::data_local_dir()
            .map(|p| p.join("rancher-desktop"))
            .ok_or(PathError::NoDataDir)
    }

    #[cfg(target_os = "linux")]
    {
        dirs::data_local_dir()
            .map(|p| p.join("rancher-desktop"))
            .ok_or(PathError::NoDataDir)
    }
}

/// Returns the current system architecture string for k3s downloads.
pub fn arch_string() -> &'static str {
    #[cfg(target_arch = "x86_64")]
    {
        "amd64"
    }

    #[cfg(target_arch = "aarch64")]
    {
        "arm64"
    }

    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        compile_error!("Unsupported architecture")
    }
}

/// Returns the k3s binary name for the current architecture.
pub fn k3s_binary_name() -> &'static str {
    #[cfg(target_arch = "x86_64")]
    {
        "k3s"
    }

    #[cfg(target_arch = "aarch64")]
    {
        "k3s-arm64"
    }

    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        compile_error!("Unsupported architecture")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_k3s_cache_dir() {
        let path = k3s_cache_dir().expect("Should get cache dir");
        assert!(path.to_string_lossy().contains("rancher-desktop"));
        assert!(path.to_string_lossy().contains("k3s"));
    }

    #[test]
    fn test_k3s_version_cache_dir() {
        let path = k3s_version_cache_dir("v1.33.3+k3s1").expect("Should get version cache dir");
        assert!(path.to_string_lossy().contains("v1.33.3+k3s1"));
    }

    #[test]
    fn test_rd_engine_path() {
        let path = rd_engine_json_path().expect("Should get rd-engine path");
        assert!(path.to_string_lossy().contains("rd-engine.json"));
    }

    #[test]
    fn test_arch_string() {
        let arch = arch_string();
        assert!(arch == "amd64" || arch == "arm64");
    }
}
