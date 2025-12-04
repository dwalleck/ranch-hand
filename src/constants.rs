//! Shared constants used across the application.

/// URL endpoints required by Rancher Desktop.
///
/// These endpoints are used for network connectivity checks and certificate validation.
/// See: <https://docs.rancherdesktop.io/getting-started/installation#proxy-environments-important-url-patterns>
pub const REQUIRED_ENDPOINTS: &[(&str, &str)] = &[
    (
        "K3s Releases API",
        "https://api.github.com/repos/k3s-io/k3s/releases",
    ),
    (
        "K3s Releases",
        "https://github.com/k3s-io/k3s/releases",
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
pub fn extract_domain(url: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|s| s.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_domain_valid_urls() {
        assert_eq!(
            extract_domain("https://api.github.com/repos/k3s-io/k3s/releases"),
            Some("api.github.com".to_string())
        );
        assert_eq!(
            extract_domain("https://github.com/k3s-io/k3s/releases"),
            Some("github.com".to_string())
        );
        assert_eq!(
            extract_domain("https://storage.googleapis.com/kubernetes-release/release"),
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
            assert!(
                domain.is_some(),
                "Endpoint '{name}' has invalid URL: {url}"
            );
        }
    }
}
