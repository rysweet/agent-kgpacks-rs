//! Build provenance + the printed next-step command — pure, directly tested.
//!
//! Ports `buildProvenance` + `buildBuildCommand` from the reference
//! `scripts/cve-corpus.mjs`. The release **tag** becomes the corpus `commit`
//! (`--corpus-commit`) and the release **date** the corpus `date`
//! (`--corpus-date`), so a pack built from a fetched corpus is traceable to the
//! exact source release. A small dependency-free UTC formatter stamps `fetched_at`.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::select::ParsedRelease;

/// Sidecar filename written next to the extracted corpus.
pub const PROVENANCE_FILENAME: &str = "corpus-provenance.json";

/// The name of the (follow-up) Rust CVE-pack builder that consumes a fetched corpus.
/// The printed command carries the provenance flags that builder will read.
pub const BUILD_COMMAND: &str = "kgpacks build-cve-pack";

/// Build provenance for the manifest, matching what the pack builder stamps: the
/// release tag becomes the corpus "commit" and the release date the corpus "date".
///
/// `fetched_at` is injected (rather than read from the clock) so the value is
/// deterministic in tests; production passes [`now_iso8601`].
pub fn build_provenance(parsed: &ParsedRelease, fetched_at: &str) -> Value {
    json!({
        "corpus": {
            "name": "cvelistV5",
            "commit": if parsed.tag.is_empty() { "unknown" } else { parsed.tag.as_str() },
            "date": parsed.corpus_date.as_deref().unwrap_or("unknown"),
            "kind": parsed.kind.as_str(),
            "asset": parsed.asset.name,
        },
        "fetched_at": fetched_at,
    })
}

/// The exact build command that consumes a fetched corpus with correct provenance.
///
/// Mirrors the reference `buildBuildCommand`, emitting `--corpus-commit`/`--corpus-date`
/// derived from the source release (and an optional `--limit`).
pub fn build_build_command(src_dir: &str, parsed: &ParsedRelease, limit: Option<u64>) -> String {
    let mut parts: Vec<String> = vec![
        BUILD_COMMAND.to_string(),
        "--src".to_string(),
        src_dir.to_string(),
    ];
    if let Some(limit) = limit {
        parts.push("--limit".to_string());
        parts.push(limit.to_string());
    }
    if !parsed.tag.is_empty() {
        parts.push("--corpus-commit".to_string());
        parts.push(parsed.tag.clone());
    }
    if let Some(date) = &parsed.corpus_date {
        parts.push("--corpus-date".to_string());
        parts.push(date.clone());
    }
    parts.join(" ")
}

/// Convert `(year, month, day)` from a count of days since the Unix epoch.
///
/// Howard Hinnant's civil-from-days algorithm (public domain), valid across the
/// full Gregorian range — used to format `fetched_at` without a date-time crate.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Format a Unix timestamp (seconds) as an ISO-8601 UTC string, `YYYY-MM-DDTHH:MM:SSZ`.
pub fn iso8601_utc(secs_since_epoch: u64) -> String {
    let days = (secs_since_epoch / 86_400) as i64;
    let rem = secs_since_epoch % 86_400;
    let (h, min, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{min:02}:{s:02}Z")
}

/// The current time as an ISO-8601 UTC string (production `fetched_at`).
///
/// Falls back to the epoch if the system clock is set before 1970.
pub fn now_iso8601() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    iso8601_utc(secs)
}
