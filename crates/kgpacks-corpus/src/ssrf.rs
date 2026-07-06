//! The SSRF gate — the pure host allowlist every fetched URL must pass.
//!
//! Ports `isAllowedDownloadHost` + `assertAllowedUrl` from the reference
//! `scripts/cve-corpus.mjs`. Acquiring data from an external service is
//! SSRF-sensitive, so the integration fails closed: HTTPS only, an allowlist of
//! GitHub hosts, and rejection of embedded credentials and literal-IP hosts. The
//! real download path re-runs [`assert_allowed_url`] on **every** redirect hop.

use url::{Host, Url};

use crate::error::{CorpusError, Result};

/// True if a download may target this host: GitHub API/web and its release-asset CDNs.
///
/// Literal IPs are never allowed (the asset URLs are always DNS names), so a hostname
/// that parses as an IP address is rejected outright — mirroring the reference's
/// `isIP(host) !== 0` guard.
pub fn is_allowed_download_host(hostname: &str) -> bool {
    if hostname.is_empty() {
        return false;
    }
    let h = hostname.to_ascii_lowercase();
    // Reject literal IPv4/IPv6 hosts (e.g. `169.254.169.254`, `127.0.0.1`, `::1`).
    if h.parse::<std::net::IpAddr>().is_ok() {
        return false;
    }
    h == "github.com"
        || h == "api.github.com"
        || h == "codeload.github.com"
        || h == "githubusercontent.com"
        || h.ends_with(".githubusercontent.com")
}

/// SSRF gate for every URL the fetcher touches (the API call, the asset URL, and each
/// redirect hop): the URL must be `https`, must not carry credentials, and must resolve
/// to a trusted GitHub host. Returns the parsed [`Url`] on success.
pub fn assert_allowed_url(raw: &str) -> Result<Url> {
    let url =
        Url::parse(raw).map_err(|_| CorpusError::with_url(format!("Malformed URL: {raw}"), raw))?;

    if url.scheme() != "https" {
        return Err(CorpusError::with_url(
            format!(
                "Only https URLs are allowed (got {})",
                if url.scheme().is_empty() {
                    "no scheme"
                } else {
                    url.scheme()
                }
            ),
            raw,
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(CorpusError::with_url(
            "URLs with embedded credentials are not allowed",
            raw,
        ));
    }

    // Literal-IP hosts parse as `Host::Ipv4`/`Host::Ipv6`; only domain hosts on the
    // allowlist are permitted.
    let allowed = matches!(url.host(), Some(Host::Domain(d)) if is_allowed_download_host(d));
    if !allowed {
        let host = url.host_str().unwrap_or("");
        return Err(CorpusError::with_url(
            format!("Host is not an allowed GitHub host: {host}"),
            raw,
        ));
    }
    Ok(url)
}
