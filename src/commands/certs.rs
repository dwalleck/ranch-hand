//! SSL certificate checking command.
//!
//! Tests connectivity to domains required by Rancher Desktop and reports
//! certificate chain information, detecting corporate proxy interception.

use crate::cli::Cli;
use crate::client::http::is_proxy_issuer;
use crate::constants::{extract_domain, REQUIRED_ENDPOINTS};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use colored::Colorize;
use rustls::pki_types::ServerName;
use serde::Serialize;
use std::sync::{Arc, Once};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tracing::{debug, info, warn};
use x509_parser::prelude::*;

/// Ensures the crypto provider is initialized exactly once
static CRYPTO_PROVIDER_INIT: Once = Once::new();

/// Connection timeout for certificate checks
const CONNECT_TIMEOUT_SECS: u64 = 10;

/// Result of checking a single domain's certificate
#[derive(Debug, Clone, Serialize)]
pub struct CertCheckResult {
    /// Domain that was checked
    pub domain: String,
    /// Whether the connection succeeded
    pub success: bool,
    /// Error message if connection failed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Certificate information if available
    #[serde(skip_serializing_if = "Option::is_none")]
    pub certificate: Option<CertificateInfo>,
    /// Whether a corporate proxy was detected
    pub proxy_detected: bool,
}

/// Information about a certificate
#[derive(Debug, Clone, Serialize)]
pub struct CertificateInfo {
    /// Certificate subject (CN)
    pub subject: String,
    /// Certificate issuer
    pub issuer: String,
    /// Not valid before (ISO 8601)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub not_before: Option<String>,
    /// Not valid after (ISO 8601)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub not_after: Option<String>,
    /// Number of certificates in chain
    pub chain_length: usize,
}

/// Output structure for the certs check command
#[derive(Debug, Serialize)]
pub struct CertsCheckOutput {
    /// Results for each domain
    pub results: Vec<CertCheckResult>,
    /// Overall status
    pub all_ok: bool,
    /// Whether any corporate proxy was detected
    pub proxy_detected: bool,
    /// Recommendations for the user
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub recommendations: Vec<String>,
}

/// Check SSL certificates for required domains
pub async fn check(cli: &Cli) -> Result<()> {
    info!("Starting certificate check for required domains");

    let show_progress = !cli.quiet && !cli.json;

    if show_progress {
        println!("{}", "SSL Certificate Check".bold().cyan());
        println!();
        println!("Checking connectivity to domains required by Rancher Desktop...");
        println!();
    }

    // Check all domains concurrently for better performance
    let futures: Vec<_> = REQUIRED_ENDPOINTS
        .iter()
        .map(|(name, url)| {
            debug!("Checking endpoint: {} ({})", name, url);
            check_endpoint(name, url, cli.insecure)
        })
        .collect();

    let results = futures_util::future::join_all(futures).await;

    let any_proxy_detected = results.iter().any(|r| r.proxy_detected);

    if show_progress {
        for result in &results {
            print_domain_result(result);
        }
    }

    let all_ok = results.iter().all(|r| r.success);
    let recommendations = generate_recommendations(&results, any_proxy_detected);

    if cli.json {
        let output = CertsCheckOutput {
            results,
            all_ok,
            proxy_detected: any_proxy_detected,
            recommendations: recommendations.clone(),
        };
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else if !cli.quiet {
        println!();
        print_summary(all_ok, any_proxy_detected, &recommendations);
    }

    Ok(())
}

/// Check a single endpoint's certificate
async fn check_endpoint(name: &str, url: &str, insecure: bool) -> CertCheckResult {
    let Some(domain) = extract_domain(url) else {
        return CertCheckResult {
            domain: name.to_string(),
            success: false,
            error: Some(format!("Invalid URL: {url}")),
            certificate: None,
            proxy_detected: false,
        };
    };

    match check_domain_inner(&domain, insecure).await {
        Ok((cert_info, proxy_detected)) => CertCheckResult {
            domain: format!("{name} ({domain})"),
            success: true,
            error: None,
            certificate: Some(cert_info),
            proxy_detected,
        },
        Err(e) => {
            warn!("Certificate check failed for {} ({}): {}", name, domain, e);
            CertCheckResult {
                domain: format!("{name} ({domain})"),
                success: false,
                error: Some(e.to_string()),
                certificate: None,
                proxy_detected: false,
            }
        }
    }
}

/// Inner function that does the actual certificate check
async fn check_domain_inner(domain: &str, insecure: bool) -> Result<(CertificateInfo, bool)> {
    // Install the ring crypto provider exactly once
    CRYPTO_PROVIDER_INIT.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });

    // Build TLS config - separate paths for insecure vs secure mode
    // In secure mode, use platform verifier to match what the OS/browser would see
    let config = if insecure {
        rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerifier))
            .with_no_client_auth()
    } else {
        // Use platform certificate verifier (Windows CryptoAPI, macOS Security.framework, etc.)
        // This matches what Electron/Chromium would see, unlike reqwest's default which uses
        // Mozilla's bundled CA certificates via webpki-roots.
        //
        // Note: Only this diagnostic tool uses platform verification. The HTTP client
        // (client/http.rs) uses reqwest's default verification, which is intentional -
        // we want the diagnostic to show what the OS sees, while the HTTP client behavior
        // matches what most Rust HTTP clients would experience.
        use rustls_platform_verifier::ConfigVerifierExt;
        rustls::ClientConfig::with_platform_verifier()
            .context("Failed to initialize platform certificate verifier")?
    };

    let connector = TlsConnector::from(Arc::new(config));

    // Connect with timeout
    let addr = format!("{domain}:443");
    let stream = tokio::time::timeout(
        Duration::from_secs(CONNECT_TIMEOUT_SECS),
        TcpStream::connect(&addr),
    )
    .await
    .with_context(|| format!("Connection to {domain} timed out"))?
    .with_context(|| format!("Failed to connect to {domain}"))?;

    let server_name = ServerName::try_from(domain.to_string())
        .map_err(|_| anyhow::anyhow!("Invalid domain name: {domain}"))?;

    let tls_stream = connector
        .connect(server_name, stream)
        .await
        .with_context(|| format!("TLS handshake failed with {domain}"))?;

    // Extract certificate information
    let (_, connection) = tls_stream.get_ref();

    let peer_certs = connection
        .peer_certificates()
        .ok_or_else(|| anyhow::anyhow!("No certificates received from {domain}"))?;

    if peer_certs.is_empty() {
        return Err(anyhow::anyhow!("Empty certificate chain from {domain}"));
    }

    // Parse the leaf certificate
    let leaf_cert = &peer_certs[0];
    let cert_info = parse_certificate(leaf_cert, peer_certs.len());

    // Check if this looks like a corporate proxy
    let proxy_detected = is_proxy_issuer(&cert_info.issuer);

    Ok((cert_info, proxy_detected))
}

/// Parse certificate DER bytes into certificate info using x509-parser
fn parse_certificate(
    cert_der: &rustls::pki_types::CertificateDer<'_>,
    chain_length: usize,
) -> CertificateInfo {
    let cert_bytes = cert_der.as_ref();

    match X509Certificate::from_der(cert_bytes) {
        Ok((_, cert)) => {
            let subject = extract_cn_or_subject(&cert.subject);
            let issuer = extract_cn_or_subject(&cert.issuer);

            let not_before = format_x509_time(&cert.validity.not_before);
            let not_after = format_x509_time(&cert.validity.not_after);

            CertificateInfo {
                subject,
                issuer,
                not_before: Some(not_before),
                not_after: Some(not_after),
                chain_length,
            }
        }
        Err(e) => {
            warn!("Failed to parse certificate: {}", e);
            CertificateInfo {
                subject: "Unable to parse certificate".to_string(),
                issuer: "Unable to parse certificate".to_string(),
                not_before: None,
                not_after: None,
                chain_length,
            }
        }
    }
}

/// Extract Common Name (CN) or full subject string from X.509 name
fn extract_cn_or_subject(name: &X509Name<'_>) -> String {
    // Try to get CN first
    for rdn in name.iter() {
        for attr in rdn.iter() {
            if attr.attr_type() == &oid_registry::OID_X509_COMMON_NAME {
                if let Ok(cn) = attr.attr_value().as_str() {
                    return cn.to_string();
                }
            }
        }
    }

    // Fall back to Organization (O)
    for rdn in name.iter() {
        for attr in rdn.iter() {
            if attr.attr_type() == &oid_registry::OID_X509_ORGANIZATION_NAME {
                if let Ok(org) = attr.attr_value().as_str() {
                    return org.to_string();
                }
            }
        }
    }

    // Last resort: convert entire name to string
    name.to_string()
}

/// Format X.509 time as ISO 8601 string
fn format_x509_time(time: &ASN1Time) -> String {
    // Convert to timestamp and then to DateTime
    let timestamp = time.timestamp();
    DateTime::<Utc>::from_timestamp(timestamp, 0).map_or_else(
        || "Invalid date".to_string(),
        |dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
    )
}

/// Print result for a single domain
fn print_domain_result(result: &CertCheckResult) {
    let status = if result.success {
        "\u{2714}".green() // ✔
    } else {
        "\u{2718}".red() // ✘
    };

    let proxy_indicator = if result.proxy_detected {
        format!(" {}", "(proxy detected)".yellow())
    } else {
        String::new()
    };

    println!("{} {}{}", status, result.domain.bold(), proxy_indicator);

    if let Some(cert) = &result.certificate {
        println!("    Subject: {}", cert.subject);
        println!("    Issuer:  {}", cert.issuer);
        if let Some(expires) = &cert.not_after {
            println!("    Expires: {expires}");
        }
        println!("    Chain:   {} certificate(s)", cert.chain_length);
    }

    if let Some(error) = &result.error {
        println!("    Error: {}", error.red());
    }

    println!();
}

/// Print summary of all results
fn print_summary(all_ok: bool, proxy_detected: bool, recommendations: &[String]) {
    println!("{}", "Summary".bold());
    println!("{}", "=".repeat(40));

    if all_ok {
        println!("{} All certificate checks passed", "\u{2714}".green());
    } else {
        println!("{} Some certificate checks failed", "\u{2718}".red());
    }

    if proxy_detected {
        println!();
        println!(
            "{} {}",
            "\u{26A0}".yellow(),
            "Corporate SSL inspection proxy detected".yellow().bold()
        );
        println!();
        println!("Your network appears to be using SSL inspection (man-in-the-middle).");
        println!("This may cause issues with Rancher Desktop downloads.");
    }

    if !recommendations.is_empty() {
        println!();
        println!("{}", "Recommendations:".yellow());
        for rec in recommendations {
            println!("  \u{2022} {rec}");
        }
    }

    println!();
}

/// Generate recommendations based on check results
fn generate_recommendations(results: &[CertCheckResult], proxy_detected: bool) -> Vec<String> {
    let mut recommendations = Vec::new();

    let failed_count = results.iter().filter(|r| !r.success).count();

    if failed_count > 0 {
        recommendations.push(format!(
            "Check your network connectivity - {failed_count} domain(s) failed"
        ));
    }

    if proxy_detected {
        recommendations
            .push("Contact your IT department to whitelist the following URLs:".to_string());
        for (name, url) in REQUIRED_ENDPOINTS {
            recommendations.push(format!("    - {name}: {url}"));
        }
        recommendations.push(
            "Alternatively, use --insecure flag (not recommended for production)".to_string(),
        );
    }

    if failed_count > 0 && !proxy_detected {
        recommendations
            .push("Try running with --insecure to bypass certificate validation".to_string());
        recommendations.push("Run 'rh diagnose' for comprehensive system diagnostics".to_string());
    }

    recommendations
}

/// Certificate verifier that accepts all certificates (for --insecure mode)
#[derive(Debug)]
struct NoVerifier;

impl rustls::client::danger::ServerCertVerifier for NoVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::RSA_PKCS1_SHA384,
            rustls::SignatureScheme::RSA_PKCS1_SHA512,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::ECDSA_NISTP521_SHA512,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512,
            rustls::SignatureScheme::ED25519,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_success_result(domain: &str, proxy: bool) -> CertCheckResult {
        CertCheckResult {
            domain: domain.to_string(),
            success: true,
            error: None,
            certificate: Some(CertificateInfo {
                subject: "example.com".to_string(),
                issuer: if proxy {
                    "Zscaler Inc".to_string()
                } else {
                    "DigiCert".to_string()
                },
                not_before: Some("2024-01-01".to_string()),
                not_after: Some("2025-01-01".to_string()),
                chain_length: 3,
            }),
            proxy_detected: proxy,
        }
    }

    fn make_failure_result(domain: &str) -> CertCheckResult {
        CertCheckResult {
            domain: domain.to_string(),
            success: false,
            error: Some("Connection failed".to_string()),
            certificate: None,
            proxy_detected: false,
        }
    }

    #[test]
    fn test_generate_recommendations_all_ok() {
        let results = vec![
            make_success_result("github.com", false),
            make_success_result("api.github.com", false),
        ];
        let recommendations = generate_recommendations(&results, false);
        assert!(recommendations.is_empty());
    }

    #[test]
    fn test_generate_recommendations_with_failures() {
        let results = vec![
            make_success_result("github.com", false),
            make_failure_result("api.github.com"),
        ];
        let recommendations = generate_recommendations(&results, false);
        assert!(!recommendations.is_empty());
        assert!(recommendations
            .iter()
            .any(|r| r.contains("1 domain(s) failed")));
    }

    #[test]
    fn test_generate_recommendations_with_proxy() {
        let results = vec![
            make_success_result("github.com", true),
            make_success_result("api.github.com", true),
        ];
        let recommendations = generate_recommendations(&results, true);
        assert!(!recommendations.is_empty());
        assert!(recommendations.iter().any(|r| r.contains("IT department")));
    }

    #[test]
    fn test_cert_check_result_serialization() {
        let result = make_success_result("github.com", false);
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("github.com"));
        assert!(json.contains("DigiCert"));
    }

    #[test]
    fn test_certs_check_output_serialization() {
        let output = CertsCheckOutput {
            results: vec![make_success_result("github.com", false)],
            all_ok: true,
            proxy_detected: false,
            recommendations: vec![],
        };
        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("all_ok"));
        assert!(json.contains("true"));
    }
}
