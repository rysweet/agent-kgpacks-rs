//! The orchestrator + injectable effect seams.
//!
//! Ports `fetchCorpus` and the default filesystem/unzip effects from the reference
//! `scripts/cve-corpus.mjs`. The network, download and unzip effects are expressed as
//! traits ([`ReleaseResolver`], [`Downloader`], [`Extractor`]) so [`fetch_corpus`] runs
//! with **zero real network / download / unzip I/O** under test (the real
//! implementations live behind the `net` feature in [`crate::net`] / [`UnzipExtractor`]).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::error::{CorpusError, Result};
use crate::provenance::{build_build_command, build_provenance, PROVENANCE_FILENAME};
use crate::select::{parse_release, CorpusKind, ParsedRelease};
use crate::ssrf::assert_allowed_url;

/// `CVEProject/cvelistV5` — the external service that publishes the corpus.
pub const CVE_REPO: &str = "CVEProject/cvelistV5";
/// The GitHub Releases API base for [`CVE_REPO`].
pub const RELEASES_API: &str = "https://api.github.com/repos/CVEProject/cvelistV5/releases";

/// Default hard download cap (3 GiB) — the baseline corpus grows over time.
pub const DEFAULT_MAX_BYTES: u64 = 3 * 1024 * 1024 * 1024;

/// Resolves a GitHub release payload for [`CVE_REPO`] (latest, or a specific tag).
pub trait ReleaseResolver {
    /// Fetch the release JSON; `tag = None` selects the latest release.
    fn get_release(&self, tag: Option<&str>) -> Result<Value>;
}

/// Streams a release asset to a local file, SSRF-guarded and byte-capped.
pub trait Downloader {
    /// Download `url` to `dest`, aborting if more than `max_bytes` are transferred.
    /// Returns the number of bytes written.
    fn download(&self, url: &str, dest: &Path, max_bytes: u64) -> Result<u64>;
}

/// Extracts a (possibly double-zipped) archive and locates the CVE record tree.
pub trait Extractor {
    /// Extract `archive` under `dest_dir` and return the corpus directory
    /// (the `cves/` or `deltaCves/` tree).
    fn extract(&self, archive: &Path, dest_dir: &Path) -> Result<PathBuf>;
}

/// Inputs to [`fetch_corpus`].
#[derive(Debug, Clone)]
pub struct FetchOptions {
    /// A specific release tag, or `None` for the latest release.
    pub tag: Option<String>,
    /// Baseline (full corpus) or delta (incremental).
    pub kind: CorpusKind,
    /// The directory to download into and extract under.
    pub dest_dir: PathBuf,
    /// A `--limit` value to echo in the printed build command.
    pub limit: Option<u64>,
    /// Keep the downloaded archive after extraction (default: delete it).
    pub keep_archive: bool,
    /// Hard download cap in bytes.
    pub max_bytes: u64,
}

impl Default for FetchOptions {
    fn default() -> Self {
        Self {
            tag: None,
            kind: CorpusKind::Baseline,
            dest_dir: PathBuf::new(),
            limit: None,
            keep_archive: false,
            max_bytes: DEFAULT_MAX_BYTES,
        }
    }
}

/// The result of a successful [`fetch_corpus`].
#[derive(Debug, Clone)]
pub struct FetchOutcome {
    /// The corpus directory to build from (the `--src` value).
    pub src_dir: PathBuf,
    /// The retained archive path, when `keep_archive` was set.
    pub archive_path: Option<PathBuf>,
    /// Bytes written during download.
    pub bytes: u64,
    /// The provenance sidecar contents (also written to disk).
    pub provenance: Value,
    /// The normalized release/asset metadata.
    pub parsed: ParsedRelease,
    /// The ready-to-run build command carrying `--corpus-commit`/`--corpus-date`.
    pub build_command: String,
}

/// Acquires the CVE corpus end-to-end: resolve a release, select + validate the asset,
/// download it, extract it, and record provenance. Every effect is injected, so this is
/// exercised under test with no real network, download, or unzip.
///
/// `now` supplies the `fetched_at` stamp (injected for deterministic tests).
pub fn fetch_corpus(
    opts: &FetchOptions,
    resolver: &dyn ReleaseResolver,
    downloader: &dyn Downloader,
    extractor: &dyn Extractor,
    now: &dyn Fn() -> String,
) -> Result<FetchOutcome> {
    if opts.dest_dir.as_os_str().is_empty() {
        return Err(CorpusError::msg("fetch_corpus requires a dest_dir"));
    }

    let release = resolver.get_release(opts.tag.as_deref())?;
    let parsed = parse_release(&release, opts.kind)?;
    // Re-validate the asset URL before any download is attempted.
    assert_allowed_url(&parsed.asset.url)?;

    fs::create_dir_all(&opts.dest_dir)?;
    let archive_path = opts.dest_dir.join(&parsed.asset.name);
    let bytes = downloader.download(&parsed.asset.url, &archive_path, opts.max_bytes)?;

    let src_dir = extractor.extract(&archive_path, &opts.dest_dir)?;

    let provenance = build_provenance(&parsed, &now());
    let mut serialized = serde_json::to_string_pretty(&provenance)?;
    serialized.push('\n');
    fs::write(opts.dest_dir.join(PROVENANCE_FILENAME), serialized)?;

    let archive_path = if opts.keep_archive {
        Some(archive_path)
    } else {
        // Best-effort cleanup — a leftover archive is not a fatal error.
        let _ = fs::remove_file(&archive_path);
        None
    };

    let build_command = build_build_command(&src_dir.to_string_lossy(), &parsed, opts.limit);

    Ok(FetchOutcome {
        src_dir,
        archive_path,
        bytes,
        provenance,
        parsed,
        build_command,
    })
}

/// Locates the directory under `root` that holds CVE records — the `cves` / `deltaCves`
/// tree if present, else the first directory containing a `CVE-YYYY-N.json` record.
/// Returns `None` when no CVE JSON is found.
pub fn find_corpus_dir(root: &Path) -> Option<PathBuf> {
    let mut stack: Vec<PathBuf> = vec![root.to_path_buf()];
    let mut fallback: Option<PathBuf> = None;

    while let Some(dir) = stack.pop() {
        if let Some(base) = dir.file_name().and_then(|n| n.to_str()) {
            if base == "cves" || base == "deltaCves" {
                return Some(dir);
            }
        }
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if is_dir {
                if name == "cves" || name == "deltaCves" {
                    return Some(path);
                }
                stack.push(path);
            } else if fallback.is_none() && is_cve_record_name(&name) {
                fallback = Some(dir.clone());
            }
        }
    }
    fallback
}

/// Whether `name` looks like a CVE record file (`CVE-YYYY-N.json`), mirroring the
/// reference `^CVE-\d{4}-\d+\.json$`.
fn is_cve_record_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("CVE-") else {
        return false;
    };
    let Some(rest) = rest.strip_suffix(".json") else {
        return false;
    };
    let Some((year, seq)) = rest.split_once('-') else {
        return false;
    };
    year.len() == 4
        && year.bytes().all(|b| b.is_ascii_digit())
        && !seq.is_empty()
        && seq.bytes().all(|b| b.is_ascii_digit())
}

/// The default extractor: runs the system `unzip`, double-unzipping when the baseline
/// asset is a `.zip.zip`, then locates the `cves/` / `deltaCves/` tree.
#[derive(Debug, Default, Clone, Copy)]
pub struct UnzipExtractor;

impl UnzipExtractor {
    /// Construct the default extractor.
    pub fn new() -> Self {
        Self
    }

    /// Runs `unzip -q -o <archive> -d <dir>`, tolerating exit 1 ("warnings").
    fn unzip_into(archive: &Path, dir: &Path) -> Result<()> {
        let output = Command::new("unzip")
            .arg("-q")
            .arg("-o")
            .arg(archive)
            .arg("-d")
            .arg(dir)
            .output()
            .map_err(|e| {
                CorpusError::msg(format!(
                    "unzip not available ({e}); install it or pre-extract the corpus"
                ))
            })?;
        // Exit 1 is a non-fatal warning (e.g. some files skipped) for our read path.
        let code = output.status.code();
        if code != Some(0) && code != Some(1) {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(CorpusError::msg(format!(
                "unzip failed ({}) for {}: {}",
                code.map(|c| c.to_string())
                    .unwrap_or_else(|| "signal".into()),
                archive.display(),
                stderr.trim()
            )));
        }
        Ok(())
    }
}

impl Extractor for UnzipExtractor {
    fn extract(&self, archive: &Path, dest_dir: &Path) -> Result<PathBuf> {
        let outer = dest_dir.join("extracted");
        let _ = fs::remove_dir_all(&outer);
        fs::create_dir_all(&outer)?;
        Self::unzip_into(archive, &outer)?;

        // A `.zip.zip` baseline extracts to a single inner `.zip`; unzip that too.
        if let Some(dir) = find_corpus_dir(&outer) {
            return Ok(dir);
        }
        let inner_zips: Vec<PathBuf> = fs::read_dir(&outer)?
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.extension()
                    .and_then(|x| x.to_str())
                    .is_some_and(|x| x.eq_ignore_ascii_case("zip"))
            })
            .collect();
        for zip in inner_zips {
            Self::unzip_into(&zip, &outer)?;
        }

        find_corpus_dir(&outer).ok_or_else(|| {
            CorpusError::msg(format!(
                "No CVE-*.json records found after extracting {}",
                archive.display()
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn touch(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    fn tool_available(cmd: &str, arg: &str) -> bool {
        Command::new(cmd)
            .arg(arg)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    #[test]
    fn is_cve_record_name_accepts_and_rejects() {
        assert!(is_cve_record_name("CVE-2026-1.json"));
        assert!(is_cve_record_name("CVE-2026-12345.json"));
        assert!(!is_cve_record_name("CVE-202-1.json")); // short year
        assert!(!is_cve_record_name("CVE-2026-.json")); // empty sequence
        assert!(!is_cve_record_name("CVE-2026-1.txt")); // wrong extension
        assert!(!is_cve_record_name("cve-2026-1.json")); // wrong case
        assert!(!is_cve_record_name("CVE-20a6-1.json")); // non-digit year
        assert!(!is_cve_record_name("CVE-2026-1a.json")); // non-digit sequence
        assert!(!is_cve_record_name("NOTCVE-2026-1.json"));
    }

    #[test]
    fn find_corpus_dir_returns_cves_when_present() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("cves")).unwrap();
        assert_eq!(
            find_corpus_dir(root.path()).unwrap(),
            root.path().join("cves")
        );
    }

    #[test]
    fn find_corpus_dir_returns_deltacves_when_present() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("deltaCves")).unwrap();
        assert_eq!(
            find_corpus_dir(root.path()).unwrap(),
            root.path().join("deltaCves")
        );
    }

    #[test]
    fn find_corpus_dir_finds_a_nested_tree() {
        let root = tempfile::tempdir().unwrap();
        let nested = root.path().join("a").join("b").join("cves");
        fs::create_dir_all(&nested).unwrap();
        assert_eq!(find_corpus_dir(root.path()).unwrap(), nested);
    }

    #[test]
    fn find_corpus_dir_falls_back_to_a_dir_with_a_record() {
        let root = tempfile::tempdir().unwrap();
        touch(&root.path().join("x").join("CVE-2026-1.json"), "{}");
        assert_eq!(find_corpus_dir(root.path()).unwrap(), root.path().join("x"));
    }

    #[test]
    fn find_corpus_dir_returns_none_without_records() {
        let root = tempfile::tempdir().unwrap();
        touch(&root.path().join("notes").join("release_notes.md"), "hi");
        assert!(find_corpus_dir(root.path()).is_none());
    }

    /// Builds `<dir>/<name>` as a zip of `sources` (paths relative to `cwd`) using the
    /// Python stdlib zipfile CLI. Returns the archive path.
    fn make_zip(cwd: &Path, name: &str, sources: &[&str]) -> PathBuf {
        let archive = cwd.join(name);
        let mut cmd = Command::new("python3");
        cmd.arg("-m")
            .arg("zipfile")
            .arg("-c")
            .arg(&archive)
            .args(sources)
            .current_dir(cwd);
        let status = cmd.status().unwrap();
        assert!(status.success(), "python3 -m zipfile failed for {name}");
        archive
    }

    #[test]
    fn extract_double_zipped_baseline_returns_the_cves_dir() {
        if !tool_available("unzip", "-v") || !tool_available("python3", "--version") {
            eprintln!("skipping: unzip/python3 not available");
            return;
        }
        let work = tempfile::tempdir().unwrap();
        // A cves/ tree with one record, zipped once...
        touch(
            &work
                .path()
                .join("cves")
                .join("2026")
                .join("CVE-2026-1234.json"),
            "{}",
        );
        make_zip(work.path(), "inner.zip", &["cves"]);
        // ...then zipped again to a .zip.zip baseline.
        let outer = make_zip(work.path(), "all_CVEs.zip.zip", &["inner.zip"]);

        let dest = tempfile::tempdir().unwrap();
        let src = UnzipExtractor::new().extract(&outer, dest.path()).unwrap();
        assert_eq!(src.file_name().unwrap(), "cves");
        assert!(src.join("2026").join("CVE-2026-1234.json").exists());
    }

    #[test]
    fn extract_single_zipped_delta_returns_the_deltacves_dir() {
        if !tool_available("unzip", "-v") || !tool_available("python3", "--version") {
            eprintln!("skipping: unzip/python3 not available");
            return;
        }
        let work = tempfile::tempdir().unwrap();
        touch(&work.path().join("deltaCves").join("CVE-2026-9.json"), "{}");
        let archive = make_zip(work.path(), "delta.zip", &["deltaCves"]);

        let dest = tempfile::tempdir().unwrap();
        let src = UnzipExtractor::new()
            .extract(&archive, dest.path())
            .unwrap();
        assert_eq!(src.file_name().unwrap(), "deltaCves");
    }

    #[test]
    fn extract_errors_when_no_cve_records_are_present() {
        if !tool_available("unzip", "-v") || !tool_available("python3", "--version") {
            eprintln!("skipping: unzip/python3 not available");
            return;
        }
        let work = tempfile::tempdir().unwrap();
        touch(&work.path().join("release_notes.md"), "notes");
        let archive = make_zip(work.path(), "notes.zip", &["release_notes.md"]);

        let dest = tempfile::tempdir().unwrap();
        let err = UnzipExtractor::new()
            .extract(&archive, dest.path())
            .unwrap_err();
        assert!(err.to_string().contains("No CVE-*.json records"));
    }
}
