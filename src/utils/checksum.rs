//! SHA256 checksum verification utilities.

use anyhow::{anyhow, Context, Result};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use thiserror::Error;

/// Buffer size for file hashing operations (64KB for better I/O performance on large files)
const HASH_BUFFER_SIZE: usize = 65536;

#[derive(Error, Debug)]
pub enum ChecksumError {
    #[error("Checksum mismatch for {filename}: expected {expected}, got {actual}")]
    Mismatch {
        filename: String,
        expected: String,
        actual: String,
    },
    #[error("No checksum found for {0}")]
    NotFound(String),
    #[error("Invalid checksum file format: {0}")]
    InvalidFormat(String),
}

/// Parse a sha256sum file into a map of filename -> hash.
///
/// Standard sha256sum format uses two spaces between hash and filename,
/// but this parser handles both single and double spaces by splitting on
/// the first space and trimming whitespace from the filename.
pub fn parse_checksum_file(content: &str) -> Result<HashMap<String, String>> {
    let mut checksums = HashMap::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        // Split on first space; extra leading spaces on filename are trimmed below
        let parts: Vec<&str> = line.splitn(2, ' ').collect();
        if parts.len() != 2 {
            return Err(ChecksumError::InvalidFormat(format!(
                "Expected 'hash  filename', got: {line}"
            ))
            .into());
        }

        let hash = parts[0].trim().to_lowercase();
        let filename = parts[1].trim().trim_start_matches('*'); // Handle binary mode marker

        // Validate hash format (64 hex characters for SHA256)
        if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(
                ChecksumError::InvalidFormat(format!("Invalid SHA256 hash: {hash}")).into(),
            );
        }

        checksums.insert(filename.to_string(), hash);
    }

    Ok(checksums)
}

/// Calculate SHA256 hash of a file.
pub fn calculate_file_hash(path: &Path) -> Result<String> {
    let mut file =
        std::fs::File::open(path).with_context(|| format!("Failed to open {}", path.display()))?;

    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; HASH_BUFFER_SIZE];

    loop {
        let bytes_read = file
            .read(&mut buffer)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }

    Ok(hex::encode(hasher.finalize()))
}

/// Verify a file against an expected hash.
pub fn verify_file(path: &Path, expected_hash: &str) -> Result<()> {
    let actual_hash = calculate_file_hash(path)?;
    let expected_lower = expected_hash.to_lowercase();

    if actual_hash != expected_lower {
        let filename = path
            .file_name()
            .ok_or_else(|| anyhow!("Invalid file path: {}", path.display()))?
            .to_string_lossy()
            .into();
        return Err(ChecksumError::Mismatch {
            filename,
            expected: expected_lower,
            actual: actual_hash,
        }
        .into());
    }

    Ok(())
}

/// Verify a file against a checksums map.
pub fn verify_file_from_checksums(path: &Path, checksums: &HashMap<String, String>) -> Result<()> {
    let filename = path
        .file_name()
        .ok_or_else(|| anyhow!("Invalid file path"))?
        .to_string_lossy();

    let expected_hash = checksums
        .get(filename.as_ref())
        .ok_or_else(|| ChecksumError::NotFound(filename.to_string()))?;

    verify_file(path, expected_hash)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_parse_checksum_file_double_space() {
        // SHA256 hashes are exactly 64 hex characters
        let content = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef  k3s\n\
                       fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210  k3s-arm64";
        let checksums = parse_checksum_file(content).unwrap();
        assert_eq!(checksums.len(), 2);
        assert_eq!(
            checksums.get("k3s"),
            Some(&"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string())
        );
    }

    #[test]
    fn test_parse_checksum_file_single_space() {
        let content = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef k3s";
        let checksums = parse_checksum_file(content).unwrap();
        assert_eq!(checksums.len(), 1);
    }

    #[test]
    fn test_parse_checksum_file_with_binary_marker() {
        let content = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef *k3s";
        let checksums = parse_checksum_file(content).unwrap();
        assert!(checksums.contains_key("k3s"));
    }

    #[test]
    fn test_parse_checksum_file_invalid_hash() {
        let content = "invalid  k3s";
        assert!(parse_checksum_file(content).is_err());
    }

    #[test]
    fn test_calculate_file_hash() {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(b"test content").unwrap();
        file.flush().unwrap();

        let hash = calculate_file_hash(file.path()).unwrap();
        // SHA256 of "test content"
        assert_eq!(
            hash,
            "6ae8a75555209fd6c44157c0aed8016e763ff435a19cf186f76863140143ff72"
        );
    }

    #[test]
    fn test_verify_file_success() {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(b"test content").unwrap();
        file.flush().unwrap();

        let result = verify_file(
            file.path(),
            "6ae8a75555209fd6c44157c0aed8016e763ff435a19cf186f76863140143ff72",
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_verify_file_mismatch() {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(b"test content").unwrap();
        file.flush().unwrap();

        let result = verify_file(
            file.path(),
            "0000000000000000000000000000000000000000000000000000000000000000",
        );
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Checksum mismatch"));
    }
}
