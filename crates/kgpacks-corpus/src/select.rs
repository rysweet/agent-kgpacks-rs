//! Release-asset selection + validation — the pure, parity-critical surface.
//!
//! Ports the selection/validation half of the reference `scripts/cve-corpus.mjs`:
//! [`corpus_date_from_tag`], [`select_baseline_asset`], [`select_delta_asset`] and
//! [`parse_release`]. Every function here is pure (zero I/O) and unit-tested directly,
//! so the contract holds without touching the network or filesystem.

use serde_json::Value;

use crate::error::{CorpusError, Result};

/// Which CVE asset to acquire from a release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CorpusKind {
    /// The double-zipped full corpus (`*_all_CVEs_*.zip.zip`, ~550 MB).
    Baseline,
    /// The incremental "records changed since the previous release" asset.
    Delta,
}

impl CorpusKind {
    /// The lowercase wire/CLI spelling (`"baseline"` / `"delta"`).
    pub fn as_str(self) -> &'static str {
        match self {
            CorpusKind::Baseline => "baseline",
            CorpusKind::Delta => "delta",
        }
    }

    /// Parse a `--kind` value; `None` for anything but `baseline`/`delta`.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "baseline" => Some(CorpusKind::Baseline),
            "delta" => Some(CorpusKind::Delta),
            _ => None,
        }
    }
}

/// The normalized fields of the chosen release asset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseAsset {
    /// The asset filename (e.g. `2026-07-02_all_CVEs_at_midnight.zip.zip`).
    pub name: String,
    /// The `browser_download_url` to fetch it from.
    pub url: String,
    /// The declared byte size, when the API reported one.
    pub size: Option<u64>,
}

/// A GitHub release normalized into the fields the fetcher needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedRelease {
    /// Which asset kind this describes.
    pub kind: CorpusKind,
    /// The release tag (e.g. `cve_2026-07-03_0000Z`); becomes `--corpus-commit`.
    pub tag: String,
    /// The release `published_at` timestamp, verbatim (may be empty).
    pub published_at: String,
    /// The derived corpus date (`YYYY-MM-DD`); becomes `--corpus-date`.
    pub corpus_date: Option<String>,
    /// The selected download asset.
    pub asset: ReleaseAsset,
}

/// Finds an embedded `YYYY-MM-DD` date anywhere in `s` (pure scan, no regex crate).
fn find_iso_date(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    // A date is exactly 10 chars: dddd-dd-dd.
    if bytes.len() < 10 {
        return None;
    }
    let is_digit = |b: u8| b.is_ascii_digit();
    for start in 0..=bytes.len() - 10 {
        let w = &bytes[start..start + 10];
        if is_digit(w[0])
            && is_digit(w[1])
            && is_digit(w[2])
            && is_digit(w[3])
            && w[4] == b'-'
            && is_digit(w[5])
            && is_digit(w[6])
            && w[7] == b'-'
            && is_digit(w[8])
            && is_digit(w[9])
        {
            // Safe: the window is all ASCII by construction.
            return Some(String::from_utf8_lossy(w).into_owned());
        }
    }
    None
}

/// Derives the corpus date (`YYYY-MM-DD`) from a release tag such as
/// `cve_2026-07-03_0000Z`. Returns `None` when no date is embedded.
pub fn corpus_date_from_tag(tag: &str) -> Option<String> {
    find_iso_date(tag)
}

/// Case-insensitively matches an asset name against a `<needle>…zip` shape,
/// mirroring the reference regexes `/_all_CVEs.*\.zip$/i` and `/_delta_CVEs.*\.zip$/i`.
fn asset_name_matches(name: &str, needle: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains(needle) && lower.ends_with(".zip")
}

/// The baseline "all CVEs" asset (double-zipped full corpus), or `None`.
pub fn select_baseline_asset(assets: &[Value]) -> Option<&Value> {
    assets.iter().find(|a| {
        a.get("name")
            .and_then(Value::as_str)
            .is_some_and(|n| asset_name_matches(n, "_all_cves"))
    })
}

/// The incremental "delta CVEs" asset (records changed since the prior release), or `None`.
pub fn select_delta_asset(assets: &[Value]) -> Option<&Value> {
    assets.iter().find(|a| {
        a.get("name")
            .and_then(Value::as_str)
            .is_some_and(|n| asset_name_matches(n, "_delta_cves"))
    })
}

/// Validates + normalizes a GitHub release object into the fields the fetcher needs.
///
/// Errors with [`CorpusError`] when the payload is not an object or the requested
/// asset (baseline vs delta) is absent / has no download URL.
pub fn parse_release(release: &Value, kind: CorpusKind) -> Result<ParsedRelease> {
    let obj = release
        .as_object()
        .ok_or_else(|| CorpusError::msg("GitHub release payload is not an object"))?;

    let tag = obj
        .get("tag_name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let published_at = obj
        .get("published_at")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let empty: Vec<Value> = Vec::new();
    let assets = obj
        .get("assets")
        .and_then(Value::as_array)
        .unwrap_or(&empty);

    let asset = match kind {
        CorpusKind::Delta => select_delta_asset(assets),
        CorpusKind::Baseline => select_baseline_asset(assets),
    };
    let asset = asset.ok_or_else(|| {
        let names: Vec<&str> = assets
            .iter()
            .filter_map(|a| a.get("name").and_then(Value::as_str))
            .collect();
        let names = if names.is_empty() {
            "(none)".to_string()
        } else {
            names.join(", ")
        };
        let tag_label = if tag.is_empty() { "(untagged)" } else { &tag };
        CorpusError::msg(format!(
            "No {} CVE asset in release {tag_label}; assets: {names}",
            kind.as_str()
        ))
    })?;

    let name = asset
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let url = asset
        .get("browser_download_url")
        .and_then(Value::as_str)
        .filter(|u| !u.is_empty())
        .ok_or_else(|| {
            CorpusError::msg(format!(
                "Selected {} asset \"{name}\" has no download URL",
                kind.as_str()
            ))
        })?
        .to_string();
    let size = asset.get("size").and_then(Value::as_u64);

    let corpus_date = corpus_date_from_tag(&tag).or_else(|| {
        if published_at.is_empty() {
            None
        } else {
            // Mirror the reference `publishedAt.slice(0, 10)` (char-safe, tolerant of
            // strings shorter than 10 characters).
            Some(published_at.chars().take(10).collect())
        }
    });

    Ok(ParsedRelease {
        kind,
        tag,
        published_at,
        corpus_date,
        asset: ReleaseAsset { name, url, size },
    })
}
