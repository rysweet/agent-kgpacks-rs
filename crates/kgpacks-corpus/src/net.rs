//! The real network effects (`net` feature): a GitHub release resolver and an asset
//! downloader built on `reqwest`'s blocking client.
//!
//! Ports `defaultGetRelease` + `defaultDownload` from the reference
//! `scripts/cve-corpus.mjs`. Redirects are followed **manually** so every hop is
//! re-validated by [`assert_allowed_url`]; a hard byte cap and a per-request timeout
//! bound the transfer. A `GITHUB_TOKEN` / `GH_TOKEN` is sent **only** to the GitHub API
//! host (never the CDN a release download redirects to), which is both safer and
//! required — the signed CDN URL rejects an `Authorization` header.

use std::io::Read;
use std::path::Path;
use std::time::Duration;

use reqwest::blocking::{Client, Response};
use reqwest::header::{ACCEPT, AUTHORIZATION, LOCATION, USER_AGENT};
use reqwest::redirect::Policy;
use reqwest::StatusCode;
use serde_json::Value;
use url::Url;

use crate::error::{CorpusError, Result};
use crate::fetch::{Downloader, ReleaseResolver, RELEASES_API};
use crate::ssrf::assert_allowed_url;

const AGENT: &str = "kgpacks-cve-corpus/1.0 (+knowledge-graph-builder)";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15 * 60); // 15 min for a ~550 MB baseline.
const MAX_REDIRECTS: usize = 5;
const COPY_BUF: usize = 64 * 1024;

/// Reads `GITHUB_TOKEN` (or `GH_TOKEN`) from the environment.
fn github_token() -> Option<String> {
    std::env::var("GITHUB_TOKEN")
        .or_else(|_| std::env::var("GH_TOKEN"))
        .ok()
        .filter(|t| !t.is_empty())
}

/// A `reqwest` blocking client with automatic redirects DISABLED (we follow them
/// manually so each hop is SSRF-re-validated) and a total per-request timeout.
fn build_client() -> Result<Client> {
    Client::builder()
        .redirect(Policy::none())
        .timeout(DEFAULT_TIMEOUT)
        .user_agent(AGENT)
        .build()
        .map_err(CorpusError::from)
}

/// Whether an `Authorization` token may be sent to this host — the GitHub API/web
/// origin only, never the `*.githubusercontent.com` CDN a download redirects to.
fn may_send_token(url: &Url) -> bool {
    matches!(url.host_str(), Some("api.github.com") | Some("github.com"))
}

/// Issues a GET to `start`, following redirects manually and re-validating every hop
/// against the SSRF allowlist. Returns the final (non-redirect) response.
fn get_following_redirects(client: &Client, start: &str, accept: &str) -> Result<Response> {
    let token = github_token();
    let mut current = start.to_string();

    for _hop in 0..=MAX_REDIRECTS {
        let url = assert_allowed_url(&current)?;
        let mut req = client
            .get(url.clone())
            .header(USER_AGENT, AGENT)
            .header(ACCEPT, accept);
        if let (Some(tok), true) = (token.as_deref(), may_send_token(&url)) {
            req = req.header(AUTHORIZATION, format!("Bearer {tok}"));
        }

        let response = req.send().map_err(|e| {
            CorpusError::with_url(
                format!("request failed for {current}: {e}"),
                current.clone(),
            )
        })?;

        let status = response.status();
        if status.is_redirection() {
            let location = response
                .headers()
                .get(LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| {
                    CorpusError::with_url(
                        format!("Redirect {status} without a Location header"),
                        current.clone(),
                    )
                })?
                .to_string();
            // Resolve relative redirects against the current URL, then re-validate.
            let base = Url::parse(&current).map_err(|_| {
                CorpusError::with_url(format!("Malformed URL: {current}"), current.clone())
            })?;
            current = base
                .join(&location)
                .map_err(|_| {
                    CorpusError::with_url(
                        format!("Malformed redirect target: {location}"),
                        current.clone(),
                    )
                })?
                .to_string();
            continue;
        }
        return Ok(response);
    }
    Err(CorpusError::with_url(
        format!("Too many redirects (> {MAX_REDIRECTS}) starting from {start}"),
        start.to_string(),
    ))
}

/// Resolves a `CVEProject/cvelistV5` release via the GitHub Releases API.
#[derive(Debug)]
pub struct GithubReleaseResolver {
    client: Client,
}

impl GithubReleaseResolver {
    /// Build a resolver with a fresh, redirect-manual HTTPS client.
    pub fn new() -> Result<Self> {
        Ok(Self {
            client: build_client()?,
        })
    }
}

impl ReleaseResolver for GithubReleaseResolver {
    fn get_release(&self, tag: Option<&str>) -> Result<Value> {
        let api_url = match tag {
            Some(t) => format!(
                "{RELEASES_API}/tags/{}",
                utf8_percent_encode_path_segment(t)
            ),
            None => format!("{RELEASES_API}/latest"),
        };
        // Validate the API URL up front (defense in depth; it is always api.github.com).
        assert_allowed_url(&api_url)?;

        let response =
            get_following_redirects(&self.client, &api_url, "application/vnd.github+json")?;
        let status = response.status();
        if !status.is_success() {
            let hint = if status == StatusCode::FORBIDDEN {
                " (rate-limited? set GITHUB_TOKEN)"
            } else {
                ""
            };
            return Err(CorpusError::with_url(
                format!("GitHub API returned HTTP {status} for {api_url}{hint}"),
                api_url,
            ));
        }
        let text = response.text().map_err(CorpusError::from)?;
        let value: Value = serde_json::from_str(&text)?;
        Ok(value)
    }
}

/// Streams a GitHub release asset to disk with a byte cap and re-validated redirects.
#[derive(Debug)]
pub struct HttpDownloader {
    client: Client,
}

impl HttpDownloader {
    /// Build a downloader with a fresh, redirect-manual HTTPS client.
    pub fn new() -> Result<Self> {
        Ok(Self {
            client: build_client()?,
        })
    }
}

impl Downloader for HttpDownloader {
    fn download(&self, url: &str, dest: &Path, max_bytes: u64) -> Result<u64> {
        let mut response = get_following_redirects(&self.client, url, "application/octet-stream")?;
        let status = response.status();
        if !status.is_success() {
            return Err(CorpusError::with_url(
                format!("Unexpected HTTP {status} downloading {url}"),
                url.to_string(),
            ));
        }

        // Reject up front if the server declares a size over the cap.
        if let Some(declared) = response.content_length() {
            if declared > max_bytes {
                return Err(CorpusError::with_url(
                    format!(
                        "Asset is {declared} bytes, exceeds cap {max_bytes}; raise --max-bytes to proceed"
                    ),
                    url.to_string(),
                ));
            }
        }

        let mut file = std::fs::File::create(dest)?;
        let mut buf = vec![0u8; COPY_BUF];
        let mut written: u64 = 0;
        loop {
            let n = response.read(&mut buf)?;
            if n == 0 {
                break;
            }
            written += n as u64;
            if written > max_bytes {
                // Drop the partial file so an over-cap asset cannot exhaust the disk.
                drop(file);
                let _ = std::fs::remove_file(dest);
                return Err(CorpusError::with_url(
                    format!("Download exceeded cap {max_bytes} bytes"),
                    url.to_string(),
                ));
            }
            std::io::Write::write_all(&mut file, &buf[..n])?;
        }
        Ok(written)
    }
}

/// Minimal path-segment percent-encoding for a release tag placed in the API URL.
///
/// Release tags are `[A-Za-z0-9._-]` in practice; this escapes anything outside that
/// safe set so a hostile tag cannot alter the request path.
fn utf8_percent_encode_path_segment(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for &b in segment.as_bytes() {
        let safe = b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~');
        if safe {
            out.push(b as char);
        } else {
            out.push('%');
            out.push_str(&format!("{b:02X}"));
        }
    }
    out
}
