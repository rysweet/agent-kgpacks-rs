//! Self-contained SemVer 2.0 helpers for pack versions (no `semver` dependency).
//!
//! Ports `packages/packs/src/versioning.ts` faithfully: the official anchored
//! SemVer 2.0 grammar (numeric core forbids leading zeros; numeric prerelease
//! identifiers forbid leading zeros; build metadata lax) gates validity, and
//! precedence follows the spec — numeric core, then prerelease-below-release,
//! then identifier-by-identifier prerelease precedence (numeric < alphanumeric,
//! numerics compared numerically), with build metadata ignored for ordering.
//! Invalid input returns [`PacksError::ManifestValidation`].

use std::cmp::Ordering;
use std::sync::OnceLock;

use regex::Regex;

use crate::errors::{PacksError, Result};

/// A parsed SemVer 2.0 version, mirroring the TypeScript `ParsedVersion`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedVersion {
    /// Major version (incompatible API changes).
    pub major: u64,
    /// Minor version (backwards-compatible feature additions).
    pub minor: u64,
    /// Patch version (backwards-compatible fixes).
    pub patch: u64,
    /// Dot-separated prerelease identifiers (empty for a release version).
    pub prerelease: Vec<String>,
    /// Dot-separated build-metadata identifiers (ignored for ordering).
    pub build: Vec<String>,
}

/// Official SemVer 2.0 grammar, anchored (same pattern as the reference).
fn semver_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-((?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*)(?:\.(?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*))*))?(?:\+([0-9a-zA-Z-]+(?:\.[0-9a-zA-Z-]+)*))?$",
        )
        .expect("valid SEMVER_RE")
    })
}

fn split_identifiers(raw: &str) -> Vec<String> {
    if raw.is_empty() {
        Vec::new()
    } else {
        raw.split('.').map(str::to_owned).collect()
    }
}

/// Whether `version` is a valid SemVer 2.0 string.
pub fn is_valid_semver(version: &str) -> bool {
    semver_re().is_match(version)
}

/// Parse `version` into its [`ParsedVersion`] components.
///
/// Errors with [`PacksError::ManifestValidation`] for any non-SemVer input, or
/// if a numeric core component is too large to fit in `u64` (the grammar permits
/// arbitrarily long numeric cores, so this is reachable on untrusted input —
/// fail cleanly rather than panic).
pub fn parse_version(version: &str) -> Result<ParsedVersion> {
    let captures = semver_re().captures(version).ok_or_else(|| {
        PacksError::ManifestValidation(format!(
            "invalid version \"{version}\" (must be valid SemVer 2.0)"
        ))
    })?;
    let group = |index: usize| captures.get(index).map(|m| m.as_str()).unwrap_or("");
    let core = |index: usize| -> Result<u64> {
        group(index).parse::<u64>().map_err(|_| {
            PacksError::ManifestValidation(format!(
                "invalid version \"{version}\": numeric component does not fit in u64"
            ))
        })
    };
    Ok(ParsedVersion {
        major: core(1)?,
        minor: core(2)?,
        patch: core(3)?,
        prerelease: split_identifiers(group(4)),
        build: split_identifiers(group(5)),
    })
}

fn is_numeric_id(id: &str) -> bool {
    !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())
}

fn compare_numeric(a: &str, b: &str) -> Ordering {
    // No leading zeros (the grammar forbids them), so longer = larger, and equal
    // lengths compare lexically — avoids any integer-overflow concern.
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}

fn compare_prerelease(a: &[String], b: &[String]) -> Ordering {
    let len = a.len().max(b.len());
    for i in 0..len {
        if i >= a.len() {
            return Ordering::Less; // a is a prefix of b -> lower precedence
        }
        if i >= b.len() {
            return Ordering::Greater;
        }
        let (x, y) = (&a[i], &b[i]);
        let order = match (is_numeric_id(x), is_numeric_id(y)) {
            (true, true) => compare_numeric(x, y),
            (true, false) => Ordering::Less, // numeric < alphanumeric
            (false, true) => Ordering::Greater,
            (false, false) => x.cmp(y),
        };
        if order != Ordering::Equal {
            return order;
        }
    }
    Ordering::Equal
}

fn compare_parsed(a: &ParsedVersion, b: &ParsedVersion) -> Ordering {
    a.major
        .cmp(&b.major)
        .then_with(|| a.minor.cmp(&b.minor))
        .then_with(|| a.patch.cmp(&b.patch))
        .then_with(
            || match (a.prerelease.is_empty(), b.prerelease.is_empty()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater, // a release outranks a prerelease
                (false, true) => Ordering::Less,
                (false, false) => compare_prerelease(&a.prerelease, &b.prerelease),
            },
        )
}

/// Compare two versions by SemVer precedence (build metadata is ignored).
///
/// Errors if either string is not valid SemVer.
pub fn compare_versions(a: &str, b: &str) -> Result<Ordering> {
    Ok(compare_parsed(&parse_version(a)?, &parse_version(b)?))
}

/// Return the input versions sorted ascending by SemVer precedence.
///
/// Every element is validated up front; an invalid version errors. The input is
/// not mutated.
pub fn sort_versions(versions: &[&str]) -> Result<Vec<String>> {
    let mut parsed = versions
        .iter()
        .map(|&v| Ok(((*v).to_owned(), parse_version(v)?)))
        .collect::<Result<Vec<_>>>()?;
    parsed.sort_by(|(_, a), (_, b)| compare_parsed(a, b));
    Ok(parsed.into_iter().map(|(version, _)| version).collect())
}

/// Return the highest-precedence version, or `None` for an empty input.
///
/// Every element is validated; an invalid version errors.
pub fn latest_version(versions: &[&str]) -> Result<Option<String>> {
    Ok(sort_versions(versions)?.pop())
}
