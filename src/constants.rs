//! Shared constants used across the application.

/// URL endpoints required by Rancher Desktop.
///
/// These endpoints are used for network connectivity checks and certificate validation.
/// See: <https://docs.rancherdesktop.io/getting-started/installation#proxy-environments-important-url-patterns>
pub const REQUIRED_ENDPOINTS: &[(&str, &str)] = &[
    // GitHub API - used to query available k3s releases
    // Uses root endpoint which returns rate limit info without auth
    ("GitHub API", "https://api.github.com"),
    // GitHub - k3s release page
    ("K3s Releases", "https://github.com/k3s-io/k3s/releases"),
    // GitHub CDN - where release assets (binaries, images) are served from
    // Base URL returns 404 but proves connectivity to the CDN
    (
        "GitHub Release Assets",
        "https://objects.githubusercontent.com",
    ),
    // GitHub raw content - checksums and other raw files
    ("GitHub Raw Content", "https://raw.githubusercontent.com"),
    // k3s update channel - used to check for k3s versions
    ("K3s Update Channel", "https://update.k3s.io"),
    // kubectl releases - stable.txt returns latest version
    (
        "kubectl Releases",
        "https://storage.googleapis.com/kubernetes-release/release/stable.txt",
    ),
    // Rancher Desktop version check API
    // Returns 405 for HEAD but proves connectivity
    (
        "Version Check",
        "https://desktop.version.rancher.io/v1/checkupgrade",
    ),
    // Rancher Desktop documentation
    ("Documentation", "https://docs.rancherdesktop.io"),
];

/// Extract domain from a URL string.
///
/// Returns `None` if the URL is malformed or has no host component.
///
/// # Examples
///
/// ```
/// use ranch_hand::constants::extract_domain;
///
/// assert_eq!(extract_domain("https://api.github.com/repos"), Some("api.github.com".to_string()));
/// assert_eq!(extract_domain("invalid-url"), None);
/// ```
#[must_use]
pub fn extract_domain(url: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(ToString::to_string))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_domain_valid_urls() {
        assert_eq!(
            extract_domain("https://api.github.com"),
            Some("api.github.com".to_string())
        );
        assert_eq!(
            extract_domain("https://github.com/k3s-io/k3s/releases"),
            Some("github.com".to_string())
        );
        assert_eq!(
            extract_domain("https://objects.githubusercontent.com"),
            Some("objects.githubusercontent.com".to_string())
        );
        assert_eq!(
            extract_domain("https://raw.githubusercontent.com"),
            Some("raw.githubusercontent.com".to_string())
        );
        assert_eq!(
            extract_domain("https://update.k3s.io"),
            Some("update.k3s.io".to_string())
        );
        assert_eq!(
            extract_domain("https://storage.googleapis.com/kubernetes-release/release/stable.txt"),
            Some("storage.googleapis.com".to_string())
        );
        assert_eq!(
            extract_domain("https://desktop.version.rancher.io/v1/checkupgrade"),
            Some("desktop.version.rancher.io".to_string())
        );
        assert_eq!(
            extract_domain("https://docs.rancherdesktop.io"),
            Some("docs.rancherdesktop.io".to_string())
        );
    }

    #[test]
    fn test_extract_domain_invalid_urls() {
        assert_eq!(extract_domain("not-a-url"), None);
        assert_eq!(extract_domain(""), None);
        assert_eq!(extract_domain("://missing-scheme"), None);
    }

    #[test]
    fn test_extract_domain_with_port() {
        assert_eq!(
            extract_domain("https://localhost:8080/api"),
            Some("localhost".to_string())
        );
    }

    #[test]
    fn test_all_required_endpoints_have_valid_domains() {
        for (name, url) in REQUIRED_ENDPOINTS {
            let domain = extract_domain(url);
            assert!(domain.is_some(), "Endpoint '{name}' has invalid URL: {url}");
        }
    }

    #[test]
    fn test_all_required_endpoints_use_https() {
        for (name, url) in REQUIRED_ENDPOINTS {
            assert!(
                url.starts_with("https://"),
                "Endpoint '{name}' must use HTTPS: {url}"
            );
        }
    }
}
