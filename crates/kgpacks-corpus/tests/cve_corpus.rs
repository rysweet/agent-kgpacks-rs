//! Contract tests for the CVE-corpus external-service integration.
//!
//! Ports `agent-kgpacks-ts` `test/cve-corpus.test.ts`: selection / validation /
//! provenance are pure and tested directly; the orchestrator [`fetch_corpus`] is
//! exercised with every network / download / unzip effect injected, so this suite
//! performs **zero real network / download / unzip I/O** (it uses a real temp dir only
//! for the provenance sidecar, exactly as the reference does).

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use kgpacks_corpus::{
    assert_allowed_url, build_build_command, build_provenance, corpus_date_from_tag, fetch_corpus,
    is_allowed_download_host, parse_release, select_baseline_asset, select_delta_asset, CorpusKind,
    Downloader, Extractor, FetchOptions, ReleaseResolver, Result, PROVENANCE_FILENAME,
};
use serde_json::{json, Value};

/// A trimmed-but-realistic cvelistV5 GitHub release payload.
fn release() -> Value {
    json!({
        "tag_name": "cve_2026-07-03_0000Z",
        "published_at": "2026-07-03T00:54:41Z",
        "assets": [
            {
                "name": "2026-07-02_all_CVEs_at_midnight.zip.zip",
                "size": 546_684_132_u64,
                "browser_download_url":
                    "https://github.com/CVEProject/cvelistV5/releases/download/cve_2026-07-03_0000Z/2026-07-02_all_CVEs_at_midnight.zip.zip"
            },
            {
                "name": "2026-07-03_delta_CVEs_at_0000Z.zip",
                "size": 22,
                "browser_download_url":
                    "https://github.com/CVEProject/cvelistV5/releases/download/cve_2026-07-03_0000Z/2026-07-03_delta_CVEs_at_0000Z.zip"
            },
            {
                "name": "release_notes.md",
                "size": 89,
                "browser_download_url":
                    "https://github.com/CVEProject/cvelistV5/releases/download/cve_2026-07-03_0000Z/release_notes.md"
            }
        ]
    })
}

fn assets(release: &Value) -> Vec<Value> {
    release["assets"].as_array().unwrap().clone()
}

// --- corpus_date_from_tag ---------------------------------------------------

#[test]
fn extracts_the_date_from_a_release_tag() {
    assert_eq!(
        corpus_date_from_tag("cve_2026-07-03_0000Z").as_deref(),
        Some("2026-07-03")
    );
}

#[test]
fn returns_none_when_no_date_is_present() {
    assert_eq!(corpus_date_from_tag("nightly"), None);
    assert_eq!(corpus_date_from_tag(""), None);
}

// --- asset selection --------------------------------------------------------

#[test]
fn picks_the_double_zipped_baseline_ignoring_delta_and_notes() {
    let a = assets(&release());
    assert_eq!(
        select_baseline_asset(&a).unwrap()["name"],
        "2026-07-02_all_CVEs_at_midnight.zip.zip"
    );
}

#[test]
fn picks_the_delta_asset() {
    let a = assets(&release());
    assert_eq!(
        select_delta_asset(&a).unwrap()["name"],
        "2026-07-03_delta_CVEs_at_0000Z.zip"
    );
}

#[test]
fn returns_none_when_no_matching_asset_exists() {
    let notes = vec![json!({ "name": "release_notes.md" })];
    assert!(select_baseline_asset(&notes).is_none());
    assert!(select_delta_asset(&[]).is_none());
}

// --- parse_release ----------------------------------------------------------

#[test]
fn normalizes_a_release_into_asset_and_provenance_fields_baseline() {
    let p = parse_release(&release(), CorpusKind::Baseline).unwrap();
    assert_eq!(p.tag, "cve_2026-07-03_0000Z");
    assert_eq!(p.corpus_date.as_deref(), Some("2026-07-03"));
    assert_eq!(p.kind, CorpusKind::Baseline);
    assert_eq!(p.asset.name, "2026-07-02_all_CVEs_at_midnight.zip.zip");
    assert!(p.asset.url.starts_with("https://github.com/"));
    assert_eq!(p.asset.size, Some(546_684_132));
}

#[test]
fn selects_the_delta_asset_when_kind_is_delta() {
    let p = parse_release(&release(), CorpusKind::Delta).unwrap();
    assert_eq!(p.asset.name, "2026-07-03_delta_CVEs_at_0000Z.zip");
}

#[test]
fn falls_back_to_published_at_for_the_corpus_date_when_the_tag_has_no_date() {
    let mut r = release();
    r["tag_name"] = json!("nightly");
    let p = parse_release(&r, CorpusKind::Baseline).unwrap();
    assert_eq!(p.corpus_date.as_deref(), Some("2026-07-03"));
}

#[test]
fn throws_when_the_requested_asset_is_missing() {
    let mut r = release();
    // Keep only release_notes.md.
    r["assets"] = json!([r["assets"][2]]);
    assert!(parse_release(&r, CorpusKind::Baseline).is_err());
}

#[test]
fn throws_on_a_non_object_payload() {
    assert!(parse_release(&Value::Null, CorpusKind::Baseline).is_err());
}

// --- SSRF gate --------------------------------------------------------------

#[test]
fn allows_github_api_web_and_githubusercontent_hosts() {
    assert!(is_allowed_download_host("github.com"));
    assert!(is_allowed_download_host("api.github.com"));
    assert!(is_allowed_download_host("objects.githubusercontent.com"));
    assert!(is_allowed_download_host(
        "release-assets.githubusercontent.com"
    ));
}

#[test]
fn rejects_other_hosts_and_literal_ips() {
    assert!(!is_allowed_download_host("evil.com"));
    assert!(!is_allowed_download_host("githubusercontent.com.evil.com"));
    assert!(!is_allowed_download_host("127.0.0.1"));
    assert!(!is_allowed_download_host("169.254.169.254"));
    assert!(!is_allowed_download_host(""));
}

#[test]
fn assert_allowed_url_accepts_https_github_urls() {
    assert!(assert_allowed_url("https://objects.githubusercontent.com/foo/bar.zip").is_ok());
}

#[test]
fn assert_allowed_url_rejects_non_https_credentials_and_off_allowlist_hosts() {
    assert!(assert_allowed_url("http://github.com/x.zip").is_err());
    assert!(assert_allowed_url("https://user:pass@github.com/x.zip").is_err());
    assert!(assert_allowed_url("https://evil.com/x.zip").is_err());
    assert!(assert_allowed_url("https://169.254.169.254/latest/meta-data").is_err());
    assert!(assert_allowed_url("not a url").is_err());
}

// --- provenance + build command --------------------------------------------

#[test]
fn maps_the_release_tag_date_onto_manifest_shaped_provenance() {
    let parsed = parse_release(&release(), CorpusKind::Baseline).unwrap();
    let prov = build_provenance(&parsed, "2026-07-03T01:20:00Z");
    assert_eq!(prov["corpus"]["name"], "cvelistV5");
    assert_eq!(prov["corpus"]["commit"], "cve_2026-07-03_0000Z");
    assert_eq!(prov["corpus"]["date"], "2026-07-03");
    assert_eq!(prov["corpus"]["kind"], "baseline");
    assert_eq!(prov["fetched_at"], "2026-07-03T01:20:00Z");
}

#[test]
fn emits_a_build_command_carrying_corpus_commit_and_corpus_date() {
    let parsed = parse_release(&release(), CorpusKind::Baseline).unwrap();
    let cmd = build_build_command("/tmp/cve/cves", &parsed, Some(500));
    assert!(cmd.contains("build-cve-pack --src /tmp/cve/cves"));
    assert!(cmd.contains("--limit 500"));
    assert!(cmd.contains("--corpus-commit cve_2026-07-03_0000Z"));
    assert!(cmd.contains("--corpus-date 2026-07-03"));
}

// --- fetch_corpus (orchestration with injected effects) ---------------------

/// A resolver that returns a fixed release payload.
struct FixedResolver(Value);
impl ReleaseResolver for FixedResolver {
    fn get_release(&self, _tag: Option<&str>) -> Result<Value> {
        Ok(self.0.clone())
    }
}

/// A downloader that records the URL it was asked to fetch and reports `bytes`.
struct RecordingDownloader {
    url: RefCell<Option<String>>,
    called: RefCell<bool>,
    bytes: u64,
}
impl RecordingDownloader {
    fn new(bytes: u64) -> Self {
        Self {
            url: RefCell::new(None),
            called: RefCell::new(false),
            bytes,
        }
    }
}
impl Downloader for RecordingDownloader {
    fn download(&self, url: &str, _dest: &Path, _max_bytes: u64) -> Result<u64> {
        *self.url.borrow_mut() = Some(url.to_string());
        *self.called.borrow_mut() = true;
        Ok(self.bytes)
    }
}

/// An extractor that records the archive path and returns a fixed corpus dir.
struct RecordingExtractor {
    archive: RefCell<Option<PathBuf>>,
    corpus: PathBuf,
}
impl Extractor for RecordingExtractor {
    fn extract(&self, archive: &Path, _dest_dir: &Path) -> Result<PathBuf> {
        *self.archive.borrow_mut() = Some(archive.to_path_buf());
        Ok(self.corpus.clone())
    }
}

fn fixed_now() -> String {
    "2026-07-03T01:20:00Z".to_string()
}

#[test]
fn resolves_downloads_extracts_and_writes_provenance() {
    let dest = tempfile::tempdir().unwrap();
    let corpus = dest.path().join("extracted").join("cves");

    let resolver = FixedResolver(release());
    let downloader = RecordingDownloader::new(42);
    let extractor = RecordingExtractor {
        archive: RefCell::new(None),
        corpus: corpus.clone(),
    };
    let opts = FetchOptions {
        kind: CorpusKind::Baseline,
        dest_dir: dest.path().to_path_buf(),
        ..FetchOptions::default()
    };

    let outcome = fetch_corpus(&opts, &resolver, &downloader, &extractor, &fixed_now).unwrap();

    // Downloaded the baseline asset URL, extracted the downloaded archive.
    assert!(downloader
        .url
        .borrow()
        .as_deref()
        .unwrap()
        .contains("all_CVEs_at_midnight.zip.zip"));
    assert_eq!(
        extractor.archive.borrow().as_deref().unwrap(),
        dest.path().join("2026-07-02_all_CVEs_at_midnight.zip.zip")
    );

    assert_eq!(outcome.src_dir, corpus);
    assert_eq!(outcome.bytes, 42);
    assert!(outcome
        .build_command
        .contains("--corpus-commit cve_2026-07-03_0000Z"));

    // Provenance sidecar is persisted next to the corpus.
    let prov_path = dest.path().join(PROVENANCE_FILENAME);
    assert!(prov_path.exists());
    let written: Value =
        serde_json::from_str(&std::fs::read_to_string(&prov_path).unwrap()).unwrap();
    assert_eq!(written["corpus"]["commit"], "cve_2026-07-03_0000Z");
    assert_eq!(written["fetched_at"], "2026-07-03T01:20:00Z");
}

#[test]
fn validates_the_asset_url_before_downloading_rejects_off_allowlist_host() {
    let dest = tempfile::tempdir().unwrap();
    let mut evil = release();
    evil["assets"][0]["browser_download_url"] = json!("https://evil.example.com/all_CVEs.zip.zip");

    let resolver = FixedResolver(evil);
    let downloader = RecordingDownloader::new(0);
    let extractor = RecordingExtractor {
        archive: RefCell::new(None),
        corpus: dest.path().to_path_buf(),
    };
    let opts = FetchOptions {
        dest_dir: dest.path().to_path_buf(),
        ..FetchOptions::default()
    };

    let result = fetch_corpus(&opts, &resolver, &downloader, &extractor, &fixed_now);
    assert!(result.is_err());
    assert!(
        !*downloader.called.borrow(),
        "must not download off-allowlist host"
    );
}

#[test]
fn requires_a_dest_dir() {
    let resolver = FixedResolver(release());
    let downloader = RecordingDownloader::new(0);
    let extractor = RecordingExtractor {
        archive: RefCell::new(None),
        corpus: PathBuf::from("/unused"),
    };
    // Default dest_dir is empty -> error before any effect runs.
    let opts = FetchOptions::default();
    let result = fetch_corpus(&opts, &resolver, &downloader, &extractor, &fixed_now);
    assert!(result.is_err());
    assert!(!*downloader.called.borrow());
}

// --- additional coverage: delta flow, keep_archive, fallbacks -----------------

/// A release whose baseline asset has a name but no download URL.
fn release_without_download_url() -> Value {
    json!({
        "tag_name": "cve_2026-07-03_0000Z",
        "assets": [
            { "name": "2026-07-02_all_CVEs_at_midnight.zip.zip" }
        ]
    })
}

/// A release with no tag and no published_at (drives the "unknown" provenance path).
fn release_without_provenance() -> Value {
    json!({
        "assets": [
            {
                "name": "2026-07-02_all_CVEs_at_midnight.zip.zip",
                "browser_download_url":
                    "https://github.com/CVEProject/cvelistV5/releases/download/x/2026-07-02_all_CVEs_at_midnight.zip.zip"
            }
        ]
    })
}

/// A downloader that actually writes bytes to `dest` (so keep/delete can be observed).
struct WritingDownloader;
impl Downloader for WritingDownloader {
    fn download(&self, _url: &str, dest: &Path, _max_bytes: u64) -> Result<u64> {
        std::fs::write(dest, b"zipbytes").unwrap();
        Ok(8)
    }
}

#[test]
fn parse_release_errors_when_selected_asset_has_no_download_url() {
    let err = parse_release(&release_without_download_url(), CorpusKind::Baseline).unwrap_err();
    assert!(err.to_string().contains("no download URL"));
}

#[test]
fn build_command_omits_absent_flags() {
    let parsed = parse_release(&release_without_provenance(), CorpusKind::Baseline).unwrap();
    let cmd = build_build_command("/tmp/cve/cves", &parsed, None);
    assert_eq!(cmd, "kgpacks build-cve-pack --src /tmp/cve/cves");
}

#[test]
fn build_provenance_uses_unknown_fallbacks() {
    let parsed = parse_release(&release_without_provenance(), CorpusKind::Baseline).unwrap();
    let prov = build_provenance(&parsed, "2026-07-03T01:20:00Z");
    assert_eq!(prov["corpus"]["commit"], "unknown");
    assert_eq!(prov["corpus"]["date"], "unknown");
}

#[test]
fn fetch_corpus_delta_flow_writes_delta_provenance() {
    let dest = tempfile::tempdir().unwrap();
    let corpus = dest.path().join("extracted").join("deltaCves");

    let resolver = FixedResolver(release());
    let downloader = RecordingDownloader::new(7);
    let extractor = RecordingExtractor {
        archive: RefCell::new(None),
        corpus: corpus.clone(),
    };
    let opts = FetchOptions {
        kind: CorpusKind::Delta,
        dest_dir: dest.path().to_path_buf(),
        ..FetchOptions::default()
    };

    let outcome = fetch_corpus(&opts, &resolver, &downloader, &extractor, &fixed_now).unwrap();
    assert!(downloader
        .url
        .borrow()
        .as_deref()
        .unwrap()
        .contains("delta"));
    assert_eq!(outcome.parsed.kind, CorpusKind::Delta);

    let prov: Value = serde_json::from_str(
        &std::fs::read_to_string(dest.path().join(PROVENANCE_FILENAME)).unwrap(),
    )
    .unwrap();
    assert_eq!(prov["corpus"]["kind"], "delta");
}

#[test]
fn fetch_corpus_keep_archive_retains_the_download() {
    let dest = tempfile::tempdir().unwrap();
    let extractor = RecordingExtractor {
        archive: RefCell::new(None),
        corpus: dest.path().join("extracted").join("cves"),
    };
    let opts = FetchOptions {
        dest_dir: dest.path().to_path_buf(),
        keep_archive: true,
        ..FetchOptions::default()
    };

    let outcome = fetch_corpus(
        &opts,
        &FixedResolver(release()),
        &WritingDownloader,
        &extractor,
        &fixed_now,
    )
    .unwrap();
    let archive = dest.path().join("2026-07-02_all_CVEs_at_midnight.zip.zip");
    assert_eq!(outcome.archive_path.as_deref(), Some(archive.as_path()));
    assert!(archive.exists(), "archive should be retained");
}

#[test]
fn fetch_corpus_deletes_the_archive_by_default() {
    let dest = tempfile::tempdir().unwrap();
    let extractor = RecordingExtractor {
        archive: RefCell::new(None),
        corpus: dest.path().join("extracted").join("cves"),
    };
    let opts = FetchOptions {
        dest_dir: dest.path().to_path_buf(),
        ..FetchOptions::default()
    };

    let outcome = fetch_corpus(
        &opts,
        &FixedResolver(release()),
        &WritingDownloader,
        &extractor,
        &fixed_now,
    )
    .unwrap();
    let archive = dest.path().join("2026-07-02_all_CVEs_at_midnight.zip.zip");
    assert!(outcome.archive_path.is_none());
    assert!(!archive.exists(), "archive should be deleted by default");
}
