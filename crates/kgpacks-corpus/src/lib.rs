//! `kgpacks-corpus` — SSRF-guarded acquisition of the CVE corpus.
//!
//! Rust port of the reference external-service integration
//! (`agent-kgpacks-ts` `scripts/cve-corpus.mjs` + `scripts/fetch-cve-corpus.mjs`,
//! PR #87). The CVE pack builder consumes a **local directory** of CVE Record Format
//! 5.1 JSON; this crate acquires that corpus from the external
//! [`CVEProject/cvelistV5`](https://github.com/CVEProject/cvelistV5) GitHub **release
//! assets** reproducibly:
//!
//! 1. Resolve a release via the GitHub API (a specific tag, else the latest; honoring
//!    `GITHUB_TOKEN`).
//! 2. Select the baseline (full, double-zipped) or delta (incremental) asset.
//! 3. Stream-download it with a **byte cap** + **timeout**, following redirects
//!    **manually and re-validating each hop** against a GitHub host allowlist.
//! 4. Double-unzip and locate the `cves/` / `deltaCves/` tree.
//! 5. Write `corpus-provenance.json` and derive build provenance (release **tag →
//!    `--corpus-commit`**, release **date → `--corpus-date`**).
//! 6. Surface the ready-to-run build command.
//!
//! The parity-critical selection / validation / provenance logic ([`select`],
//! [`ssrf`], [`provenance`]) is pure and unit-tested directly; the network, download and
//! unzip effects are injectable seams ([`ReleaseResolver`] / [`Downloader`] /
//! [`Extractor`]) so [`fetch_corpus`] is exercised with **zero real I/O**. The real
//! reqwest-backed effects live behind the opt-in [`net`](crate::net) feature so the
//! default gates stay lean/offline.

pub mod error;
pub mod fetch;
pub mod provenance;
pub mod select;
pub mod ssrf;

#[cfg(feature = "net")]
pub mod net;

pub use error::{CorpusError, Result};
pub use fetch::{
    fetch_corpus, find_corpus_dir, Downloader, Extractor, FetchOptions, FetchOutcome,
    ReleaseResolver, UnzipExtractor, CVE_REPO, DEFAULT_MAX_BYTES, RELEASES_API,
};
pub use provenance::{
    build_build_command, build_provenance, iso8601_utc, now_iso8601, BUILD_COMMAND,
    PROVENANCE_FILENAME,
};
pub use select::{
    corpus_date_from_tag, parse_release, select_baseline_asset, select_delta_asset, CorpusKind,
    ParsedRelease, ReleaseAsset,
};
pub use ssrf::{assert_allowed_url, is_allowed_download_host};

#[cfg(feature = "net")]
pub use net::{GithubReleaseResolver, HttpDownloader};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corpus_date_extracts_and_rejects() {
        assert_eq!(
            corpus_date_from_tag("cve_2026-07-03_0000Z").as_deref(),
            Some("2026-07-03")
        );
        assert_eq!(corpus_date_from_tag("nightly"), None);
        assert_eq!(corpus_date_from_tag(""), None);
    }

    #[test]
    fn corpus_kind_round_trips() {
        assert_eq!(CorpusKind::parse("baseline"), Some(CorpusKind::Baseline));
        assert_eq!(CorpusKind::parse("delta"), Some(CorpusKind::Delta));
        assert_eq!(CorpusKind::parse("other"), None);
        assert_eq!(CorpusKind::Baseline.as_str(), "baseline");
        assert_eq!(CorpusKind::Delta.as_str(), "delta");
    }

    #[test]
    fn iso8601_formats_a_known_instant() {
        // 2026-07-03T01:20:00Z == 1_783_041_600 seconds since the epoch.
        assert_eq!(iso8601_utc(1_783_041_600), "2026-07-03T01:20:00Z");
        // The epoch itself.
        assert_eq!(iso8601_utc(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn allowlist_accepts_github_and_rejects_others() {
        for host in [
            "github.com",
            "api.github.com",
            "codeload.github.com",
            "githubusercontent.com",
            "objects.githubusercontent.com",
            "release-assets.githubusercontent.com",
        ] {
            assert!(is_allowed_download_host(host), "should allow {host}");
        }
        for host in [
            "evil.com",
            "githubusercontent.com.evil.com",
            "127.0.0.1",
            "169.254.169.254",
            "",
        ] {
            assert!(!is_allowed_download_host(host), "should reject {host}");
        }
    }
}
