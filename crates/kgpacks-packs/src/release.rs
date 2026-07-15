//! Pack release: versioned release tags + provenance mirror, and the multi-part
//! release index.
//!
//! This module ports two halves of the pack-release tool:
//!
//! # Versioned release tags + provenance derivation
//!
//! Ports the pack-publish half of WS3 — `scripts/release-pack.mjs` and
//! `packVersionFromReleaseTag` ([`crate::versioning`]) — as pure, offline,
//! fully-tested primitives:
//!
//! * A pack is published to an **immutable dated tag** `<name>-YYYY.MM[.N]`
//!   whose SemVer version is derived UNPADDED ([`resolve_release_version`] over
//!   [`crate::versioning::pack_version_from_release_tag`]), alongside a **stable
//!   `packs` latest-pointer** ([`publish_targets`]) so the `pack pull` UX always
//!   resolves the newest version ([`latest_release_tag`]).
//! * The `<name>.pack-release.json` index ([`PackReleaseIndex`]) mirrors the
//!   pack `manifest.json` build `provenance` block ([`build_release_provenance`])
//!   so the two can be cross-checked, filling gaps from CLI overrides and the
//!   release-time `build.date`.
//!
//! [`plan_release`] is the offline projection the CLI `pack release-plan`
//! command surfaces (the network-free half of `release-pack.mjs --dry-run`).
//!
//! # Multi-part release index
//!
//! A pack that exceeds [`MAX_SINGLE_ARTIFACT_BYTES`] (2 GiB) is published as an
//! ordered set of fixed-size parts. [`plan_multipart_release`] is the single
//! source of truth for that format — every non-final part is exactly
//! `part_size` bytes, `sum(parts.bytes) == total_bytes`, each part carries the
//! SHA-256 of its own bytes, and the index carries the SHA-256 of the whole
//! artifact. Size accounting ([`part_accounting`]) is pure `u64` arithmetic that
//! never materializes the artifact, so the >2 GiB path is unit-testable without
//! allocating gigabytes.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value};

use crate::errors::{PacksError, Result};
use crate::manifest::{load_manifest_from_dir, validate_and_sanitize_provenance, PackManifest};
use crate::sha256::sha256_hex;
use crate::versioning::{compare_versions, pack_version_from_release_tag};

/// The stable latest-pointer tag. `pack pull` defaults to this tag, which every
/// dated release also moves so it always points at the newest version.
pub const LATEST_POINTER_TAG: &str = "packs";

/// The release-index `format` discriminator (matches `release-pack.mjs`).
pub const RELEASE_INDEX_FORMAT: &str = "tar.gz-multipart-v1";

/// The `<name>.pack-release.json` index filename for a pack.
pub fn pack_release_filename(pack: &str) -> String {
    format!("{pack}.pack-release.json")
}

/// The `<name>.tar.gz.NNN` filename for the `index`-th part of a multi-part pack
/// release (zero-based, 3-digit zero-padded ordinal — matches `release-pack.mjs`
/// and the `<name>.tar.gz.000` / `.001` reference layout).
///
/// This is the single source of the part-file naming, shared by the planner's
/// [`MultiPartIndex::to_release_parts`] and the pull-facing [`ReleasePart`], so
/// the two representations cannot disagree on part filenames.
pub fn pack_part_filename(pack: &str, index: u64) -> String {
    format!("{pack}.tar.gz.{index:03}")
}

/// The ordered set of tags a pack release publishes to.
///
/// Ports `release-pack.mjs`'s `publishTo(tag); if (tag !== 'packs') publishTo('packs')`:
/// the requested tag first (immutable when dated), then the stable `packs`
/// latest-pointer so `pack pull` (which defaults to `packs`) always resolves the
/// newest version. Publishing directly to `packs` yields just `["packs"]`.
pub fn publish_targets(tag: &str) -> Vec<String> {
    let mut targets = vec![tag.to_string()];
    if tag != LATEST_POINTER_TAG {
        targets.push(LATEST_POINTER_TAG.to_string());
    }
    targets
}

/// The version a release publishes under for `tag`.
///
/// A dated tag pins its derived (unpadded SemVer) version; any non-dated tag
/// (the `packs` pointer, `cve`, an invalid month, …) falls back to the manifest
/// version. Ports `release-pack.mjs`'s `deriveVersionFromTag(tag) ?? manifest.version`.
pub fn resolve_release_version(tag: &str, manifest_version: &str) -> String {
    pack_version_from_release_tag(tag).unwrap_or_else(|_| manifest_version.to_string())
}

/// The newest dated release tag by derived SemVer precedence — the "resolve
/// latest" a `pack pull` performs over a set of candidate tags.
///
/// Non-dated tags (e.g. the `packs` pointer or `cve-latest`) carry no dated
/// version and are ignored. Returns `None` when no tag is dated. Errors only if
/// a derived version is not valid SemVer (unreachable for well-formed tags).
pub fn latest_release_tag<'a>(tags: &[&'a str]) -> Result<Option<&'a str>> {
    let mut best: Option<(&'a str, String)> = None;
    for &tag in tags {
        let version = match pack_version_from_release_tag(tag) {
            Ok(version) => version,
            Err(_) => continue,
        };
        let replace = match &best {
            None => true,
            Some((_, current)) => {
                compare_versions(&version, current)? == std::cmp::Ordering::Greater
            }
        };
        if replace {
            best = Some((tag, version));
        }
    }
    Ok(best.map(|(tag, _)| tag))
}

/// Overrides for building the release-index provenance (mirroring the
/// `release-pack.mjs` `--corpus-commit` / `--corpus-date` / `--model` flags).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProvenanceOverrides {
    /// Override `provenance.corpus.commit` (the cvelistV5 git commit SHA).
    pub corpus_commit: Option<String>,
    /// Override `provenance.corpus.date` (the corpus checkout date).
    pub corpus_date: Option<String>,
    /// Override the resolved embedding/synthesis model id.
    pub model: Option<String>,
}

/// Whether a JSON field is absent, `null`, or an empty string — the fields
/// `release-pack.mjs` treats as falsy (`!field`) and therefore fills with a
/// default.
fn is_absent_or_empty(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => true,
        Some(Value::String(s)) => s.is_empty(),
        Some(_) => false,
    }
}

/// The model id for a release: `overrides.model`, else the manifest's `model` /
/// `synthesis_model` field. Mirrors `modelArg ?? manifest.model ?? manifest.synthesis_model`
/// (restricted to string values, since a non-string model is meaningless here).
pub fn resolve_model(manifest: &PackManifest, overrides: &ProvenanceOverrides) -> Option<String> {
    if let Some(model) = &overrides.model {
        return Some(model.clone());
    }
    for key in ["model", "synthesis_model"] {
        if let Some(Value::String(model)) = manifest.extra.get(key) {
            return Some(model.clone());
        }
    }
    None
}

/// Build the release-index `provenance` object by mirroring the manifest's
/// provenance and filling gaps from `overrides` + the release-time `build.date`.
///
/// Ports `release-pack.mjs`'s `buildProvenance`: shallow-merge each of the
/// `corpus` / `embedding` / `build` sections from the manifest provenance,
/// override `corpus.{commit,date}` when given, default a falsy `embedding.model`
/// to the resolved model, and default a falsy `build.date` to `now_iso`. Only
/// non-empty sections are emitted; an all-empty result is `None`.
///
/// The manifest provenance was validated + deep-sanitized when the manifest was
/// loaded, so mirroring it needs no re-validation.
pub fn build_release_provenance(
    manifest: &PackManifest,
    overrides: &ProvenanceOverrides,
    now_iso: &str,
) -> Option<Value> {
    let base = manifest.provenance.as_ref().and_then(Value::as_object);
    let section = |name: &str| -> Map<String, Value> {
        base.and_then(|base| base.get(name))
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default()
    };

    let mut corpus = section("corpus");
    if let Some(commit) = &overrides.corpus_commit {
        corpus.insert("commit".into(), Value::String(commit.clone()));
    }
    if let Some(date) = &overrides.corpus_date {
        corpus.insert("date".into(), Value::String(date.clone()));
    }

    let model = resolve_model(manifest, overrides);
    let mut embedding = section("embedding");
    if let Some(model) = &model {
        if is_absent_or_empty(embedding.get("model")) {
            embedding.insert("model".into(), Value::String(model.clone()));
        }
    }

    let mut build = section("build");
    if is_absent_or_empty(build.get("date")) {
        build.insert("date".into(), Value::String(now_iso.to_string()));
    }

    let mut provenance = Map::new();
    if !corpus.is_empty() {
        provenance.insert("corpus".into(), Value::Object(corpus));
    }
    if !embedding.is_empty() {
        provenance.insert("embedding".into(), Value::Object(embedding));
    }
    if !build.is_empty() {
        provenance.insert("build".into(), Value::Object(build));
    }
    if provenance.is_empty() {
        None
    } else {
        Some(Value::Object(provenance))
    }
}

/// A single part of a multipart pack-release artifact
/// (`<name>.tar.gz.NNN`), with its byte length and SHA-256 checksum.
#[derive(Debug, Clone, PartialEq)]
pub struct ReleasePart {
    /// The part filename (`<name>.tar.gz.NNN`).
    pub file: String,
    /// The part's byte length.
    pub bytes: u64,
    /// The part's SHA-256 checksum (lowercase hex).
    pub sha256: String,
}

impl ReleasePart {
    fn to_value(&self) -> Value {
        let mut map = Map::new();
        map.insert("file".into(), Value::String(self.file.clone()));
        map.insert("bytes".into(), Value::Number(self.bytes.into()));
        map.insert("sha256".into(), Value::String(self.sha256.clone()));
        Value::Object(map)
    }

    fn from_value(value: &Value) -> Result<Self> {
        let object = value.as_object().ok_or_else(|| {
            PacksError::ManifestValidation("release part must be an object".into())
        })?;
        Ok(Self {
            file: string_field(object, "file")?,
            bytes: u64_field(object, "bytes")?,
            sha256: string_field(object, "sha256")?,
        })
    }
}

/// The `<name>.pack-release.json` index describing a published pack artifact.
///
/// Mirrors the index `release-pack.mjs` writes: identity (`name` / `version`),
/// the multipart `format`, the resolved `model`, the mirrored build
/// `provenance`, the `created_at` timestamp, the overall gzip-stream `sha256`,
/// `total_bytes`, the `part_size`, and the ordered `parts`. Serialized with the
/// reference's camelCase keys (`createdAt` / `totalBytes` / `partSize`) so a
/// Rust-written index is byte-compatible with the reference consumer.
#[derive(Debug, Clone, PartialEq)]
pub struct PackReleaseIndex {
    /// Pack name (from the manifest).
    pub name: String,
    /// Published version (dated tag → derived SemVer, else manifest version).
    pub version: String,
    /// Artifact format discriminator ([`RELEASE_INDEX_FORMAT`]).
    pub format: String,
    /// Resolved model id, if any.
    pub model: Option<String>,
    /// Mirrored build provenance, if any.
    pub provenance: Option<Value>,
    /// Index creation timestamp (ISO-8601 UTC).
    pub created_at: String,
    /// SHA-256 of the whole gzip stream (lowercase hex).
    pub sha256: String,
    /// Total gzip-stream byte length across all parts.
    pub total_bytes: u64,
    /// The fixed part size the stream was split at.
    pub part_size: u64,
    /// The ordered artifact parts.
    pub parts: Vec<ReleasePart>,
}

impl PackReleaseIndex {
    /// Serialize to the canonical `<name>.pack-release.json` JSON object.
    pub fn to_value(&self) -> Value {
        let mut map = Map::new();
        map.insert("name".into(), Value::String(self.name.clone()));
        map.insert("version".into(), Value::String(self.version.clone()));
        map.insert("format".into(), Value::String(self.format.clone()));
        if let Some(model) = &self.model {
            map.insert("model".into(), Value::String(model.clone()));
        }
        if let Some(provenance) = &self.provenance {
            map.insert("provenance".into(), provenance.clone());
        }
        map.insert("createdAt".into(), Value::String(self.created_at.clone()));
        map.insert("sha256".into(), Value::String(self.sha256.clone()));
        map.insert("totalBytes".into(), Value::Number(self.total_bytes.into()));
        map.insert("partSize".into(), Value::Number(self.part_size.into()));
        map.insert(
            "parts".into(),
            Value::Array(self.parts.iter().map(ReleasePart::to_value).collect()),
        );
        Value::Object(map)
    }

    /// Validate an arbitrary JSON value as a [`PackReleaseIndex`].
    ///
    /// Errors with [`PacksError::ManifestValidation`] on any missing/mistyped
    /// field. A present `provenance` block is validated + deep-sanitized through
    /// the same gate as a manifest's, so a malformed release index is rejected
    /// exactly as a malformed manifest is.
    pub fn from_value(value: &Value) -> Result<Self> {
        let object = value.as_object().ok_or_else(|| {
            PacksError::ManifestValidation("release index must be a JSON object".into())
        })?;

        let model = match object.get("model") {
            None | Some(Value::Null) => None,
            Some(Value::String(model)) => Some(model.clone()),
            Some(_) => {
                return Err(PacksError::ManifestValidation(
                    "release index model must be a string".into(),
                ))
            }
        };

        let provenance = match object.get("provenance") {
            None | Some(Value::Null) => None,
            Some(provenance) => Some(validate_and_sanitize_provenance(provenance)?),
        };

        let parts = match object.get("parts") {
            Some(Value::Array(items)) => items
                .iter()
                .map(ReleasePart::from_value)
                .collect::<Result<Vec<_>>>()?,
            _ => {
                return Err(PacksError::ManifestValidation(
                    "release index parts must be an array".into(),
                ))
            }
        };

        Ok(Self {
            name: string_field(object, "name")?,
            version: string_field(object, "version")?,
            format: string_field(object, "format")?,
            model,
            provenance,
            created_at: string_field(object, "createdAt")?,
            sha256: string_field(object, "sha256")?,
            total_bytes: u64_field(object, "totalBytes")?,
            part_size: u64_field(object, "partSize")?,
            parts,
        })
    }

    /// Parse and validate an index from a JSON string.
    pub fn parse_str(raw: &str) -> Result<Self> {
        let parsed: Value = serde_json::from_str(raw).map_err(|err| {
            PacksError::ManifestValidation(format!("release index is not valid JSON: {err}"))
        })?;
        Self::from_value(&parsed)
    }

    /// Write the index as pretty JSON (trailing newline) to `path`.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let mut json = serde_json::to_string_pretty(&self.to_value()).map_err(|err| {
            PacksError::ManifestValidation(format!("cannot serialize release index: {err}"))
        })?;
        json.push('\n');
        std::fs::write(path, json)?;
        Ok(())
    }

    /// Read, parse and validate an index from a file path.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let raw = std::fs::read_to_string(path).map_err(|err| {
            PacksError::ManifestValidation(format!(
                "cannot read release index at {}: {err}",
                path.display()
            ))
        })?;
        Self::parse_str(&raw)
    }
}

/// Assemble a [`PackReleaseIndex`] from a manifest, a target `tag`, and the
/// byte-level packaging results (`sha256` / `total_bytes` / `part_size` /
/// `parts`).
///
/// Resolves the published version + model, and mirrors the manifest provenance
/// (using `created_at` for both the index timestamp and a defaulted
/// `build.date`), matching the tail of `release-pack.mjs`'s `buildParts`.
#[allow(clippy::too_many_arguments)]
pub fn build_release_index(
    manifest: &PackManifest,
    tag: &str,
    overrides: &ProvenanceOverrides,
    created_at: &str,
    sha256: impl Into<String>,
    total_bytes: u64,
    part_size: u64,
    parts: Vec<ReleasePart>,
) -> PackReleaseIndex {
    PackReleaseIndex {
        name: manifest.name.clone(),
        version: resolve_release_version(tag, &manifest.version),
        format: RELEASE_INDEX_FORMAT.to_string(),
        model: resolve_model(manifest, overrides),
        provenance: build_release_provenance(manifest, overrides, created_at),
        created_at: created_at.to_string(),
        sha256: sha256.into(),
        total_bytes,
        part_size,
        parts,
    }
}

/// The offline, network-free plan for publishing a pack release: the resolved
/// version, mirrored provenance, publish targets (dated tag + `packs` pointer),
/// and the release-index filename.
///
/// This is the pure projection the CLI `pack release-plan` surfaces and
/// `release-pack.mjs --dry-run` computes before packaging + upload.
#[derive(Debug, Clone, PartialEq)]
pub struct ReleasePlan {
    /// Pack name (from the manifest).
    pub name: String,
    /// The requested release tag.
    pub tag: String,
    /// The version the release publishes under.
    pub version: String,
    /// The resolved model id, if any.
    pub model: Option<String>,
    /// The mirrored build provenance, if any.
    pub provenance: Option<Value>,
    /// The tags the release publishes to (dated tag + `packs` latest-pointer).
    pub publish_targets: Vec<String>,
    /// The `<name>.pack-release.json` index filename.
    pub index_filename: String,
}

impl ReleasePlan {
    /// Serialize to a JSON object with the CLI's camelCase keys.
    pub fn to_value(&self) -> Value {
        let mut map = Map::new();
        map.insert("name".into(), Value::String(self.name.clone()));
        map.insert("tag".into(), Value::String(self.tag.clone()));
        map.insert("version".into(), Value::String(self.version.clone()));
        map.insert(
            "model".into(),
            self.model.clone().map_or(Value::Null, Value::String),
        );
        map.insert(
            "provenance".into(),
            self.provenance.clone().unwrap_or(Value::Null),
        );
        map.insert(
            "publishTargets".into(),
            Value::Array(
                self.publish_targets
                    .iter()
                    .map(|t| Value::String(t.clone()))
                    .collect(),
            ),
        );
        map.insert(
            "indexFilename".into(),
            Value::String(self.index_filename.clone()),
        );
        Value::Object(map)
    }
}

/// Compute the offline [`ReleasePlan`] for publishing the pack in `pack_dir`
/// under `tag`.
///
/// Reads + validates the pack's `manifest.json` only (no graph store, no
/// network): resolves the (dated) version, mirrors provenance, and computes the
/// publish targets. `now_iso` defaults a missing `provenance.build.date`.
///
/// The `<name>.pack-release.json` `index_filename` is keyed off the pack
/// **directory** name (the `--pack` arg), matching `release-pack.mjs`, which
/// writes the index + part assets under that name; the plan/index `name` field
/// carries the manifest name. The two coincide when the pack directory is named
/// after its manifest, but a renamed directory keeps the on-disk asset names
/// stable.
pub fn plan_release(
    pack_dir: impl AsRef<Path>,
    tag: &str,
    overrides: &ProvenanceOverrides,
    now_iso: &str,
) -> Result<ReleasePlan> {
    let pack_dir = pack_dir.as_ref();
    let manifest = load_manifest_from_dir(pack_dir)?;
    // The pack directory name is the `--pack` arg the release tooling keys asset
    // filenames off; fall back to the manifest name for an unnameable path.
    let asset_name = pack_dir
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| manifest.name.clone());
    Ok(ReleasePlan {
        name: manifest.name.clone(),
        tag: tag.to_string(),
        version: resolve_release_version(tag, &manifest.version),
        model: resolve_model(&manifest, overrides),
        provenance: build_release_provenance(&manifest, overrides, now_iso),
        publish_targets: publish_targets(tag),
        index_filename: pack_release_filename(&asset_name),
    })
}

/// The current UTC time as an ISO-8601 `YYYY-MM-DDTHH:MM:SSZ` string, computed
/// without a datetime dependency (mirrors `new Date().toISOString()` to second
/// precision). Used to stamp `build.date` / `createdAt` at release time.
pub fn now_iso8601_utc() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    iso8601_utc_from_unix(secs)
}

/// Format a Unix timestamp (seconds) as ISO-8601 UTC `YYYY-MM-DDTHH:MM:SSZ`.
pub fn iso8601_utc_from_unix(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let seconds_of_day = secs.rem_euclid(86_400);
    let (hours, minutes, seconds) = (
        seconds_of_day / 3600,
        (seconds_of_day % 3600) / 60,
        seconds_of_day % 60,
    );
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}Z")
}

/// Convert a count of days since the Unix epoch to a `(year, month, day)` civil
/// date (Howard Hinnant's proleptic-Gregorian algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32; // [1, 12]
    let year = if month <= 2 { year + 1 } else { year };
    (year, month, day)
}

fn string_field(object: &Map<String, Value>, field: &str) -> Result<String> {
    match object.get(field) {
        Some(Value::String(value)) => Ok(value.clone()),
        _ => Err(PacksError::ManifestValidation(format!(
            "release index {field} must be a string"
        ))),
    }
}

fn u64_field(object: &Map<String, Value>, field: &str) -> Result<u64> {
    object.get(field).and_then(Value::as_u64).ok_or_else(|| {
        PacksError::ManifestValidation(format!(
            "release index {field} must be a non-negative integer"
        ))
    })
}

/// Above this single-artifact size (2 GiB), a pack MUST be published as a
/// multi-part release rather than a single blob.
pub const MAX_SINGLE_ARTIFACT_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Whether an artifact of `total_bytes` must be split into multiple parts.
pub fn requires_multipart(total_bytes: u64) -> bool {
    total_bytes > MAX_SINGLE_ARTIFACT_BYTES
}

/// Compact, allocation-free accounting for splitting `total_bytes` into
/// `part_size` chunks.
///
/// This is the >2 GiB-safe path: it computes part counts and the final-part
/// size with `u64` arithmetic and holds no artifact bytes, so it is valid for
/// multi-gigabyte totals (and for tiny `part_size` values that would imply
/// billions of parts — no per-part vector is allocated).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartAccounting {
    /// Total artifact size in bytes.
    pub total_bytes: u64,
    /// Fixed size of every non-final part.
    pub part_size: u64,
    /// Number of parts (`0` iff `total_bytes == 0`).
    pub num_parts: u64,
    /// Size of the final part (`1..=part_size`, or `0` iff `total_bytes == 0`).
    pub last_part_bytes: u64,
}

impl PartAccounting {
    /// The byte size of part `index` (0-based). Returns `None` if out of range.
    pub fn part_bytes(&self, index: u64) -> Option<u64> {
        if index >= self.num_parts {
            return None;
        }
        Some(if index + 1 == self.num_parts {
            self.last_part_bytes
        } else {
            self.part_size
        })
    }
}

/// Compute [`PartAccounting`] for `total_bytes` split into `part_size` chunks.
///
/// Errors ([`PacksError::PackInstall`]) if `part_size == 0`.
pub fn part_accounting(total_bytes: u64, part_size: u64) -> Result<PartAccounting> {
    if part_size == 0 {
        return Err(PacksError::PackInstall(
            "part_size must be greater than zero".into(),
        ));
    }
    if total_bytes == 0 {
        return Ok(PartAccounting {
            total_bytes: 0,
            part_size,
            num_parts: 0,
            last_part_bytes: 0,
        });
    }
    // Ceiling division without `total_bytes + part_size` overflow.
    let num_parts = (total_bytes - 1) / part_size + 1;
    let last_part_bytes = total_bytes - (num_parts - 1) * part_size;
    Ok(PartAccounting {
        total_bytes,
        part_size,
        num_parts,
        last_part_bytes,
    })
}

/// One part of a multi-part pack release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartEntry {
    /// 0-based ordinal of this part.
    pub index: u64,
    /// Size of this part in bytes.
    pub bytes: u64,
    /// Lowercase-hex SHA-256 of this part's bytes.
    pub sha256: String,
}

/// The multi-part release index for a single pack artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultiPartIndex {
    /// Fixed size of every non-final part.
    pub part_size: u64,
    /// Total artifact size (== `sum(parts.bytes)`).
    pub total_bytes: u64,
    /// Lowercase-hex SHA-256 of the whole artifact (== hash of the parts
    /// concatenated in order).
    pub sha256: String,
    /// The parts, in order.
    pub parts: Vec<PartEntry>,
}

impl MultiPartIndex {
    /// Serialize to the planner's structural multi-part shape (snake_case keys,
    /// `parts` carrying their 0-based `index`).
    ///
    /// NOTE: this is the *planner's* accounting projection, **not** the on-disk
    /// `<name>.pack-release.json` that `pack pull` reads — that canonical index
    /// is [`PackReleaseIndex`] (camelCase `partSize`/`totalBytes`, parts keyed by
    /// `file`). The two are kept from drifting by [`Self::to_release_parts`],
    /// which maps this planner's per-part accounting onto the pull-facing
    /// [`ReleasePart`]s, so the same byte-split feeds both representations.
    pub fn to_value(&self) -> serde_json::Value {
        serde_json::json!({
            "part_size": self.part_size,
            "total_bytes": self.total_bytes,
            "sha256": self.sha256,
            "parts": self
                .parts
                .iter()
                .map(|p| serde_json::json!({
                    "index": p.index,
                    "bytes": p.bytes,
                    "sha256": p.sha256,
                }))
                .collect::<Vec<_>>(),
        })
    }

    /// Project the planner's parts onto the pull-facing [`ReleasePart`] list for
    /// pack `pack`, assigning each its `<pack>.tar.gz.NNN` filename
    /// ([`pack_part_filename`]) while preserving its byte length and SHA-256.
    ///
    /// This is the bridge that makes [`plan_multipart_release`] the single source
    /// of the byte-split feeding the real [`PackReleaseIndex`] (via
    /// [`build_release_index`]) that `pack pull` verifies — so the planner's
    /// accounting and the on-disk release index cannot drift apart.
    pub fn to_release_parts(&self, pack: &str) -> Vec<ReleasePart> {
        self.parts
            .iter()
            .map(|p| ReleasePart {
                file: pack_part_filename(pack, p.index),
                bytes: p.bytes,
                sha256: p.sha256.clone(),
            })
            .collect()
    }
}

/// Plan a multi-part release over `data` split into `part_size`-byte chunks —
/// the real release-index computation, run dry (nothing is published).
///
/// Computes the per-part and overall SHA-256 digests so the returned
/// [`MultiPartIndex`] is byte-for-byte what a publish would record. Errors
/// ([`PacksError::PackInstall`]) if `part_size == 0`.
pub fn plan_multipart_release(data: &[u8], part_size: u64) -> Result<MultiPartIndex> {
    let accounting = part_accounting(data.len() as u64, part_size)?;

    let chunk = usize::try_from(part_size).map_err(|_| {
        PacksError::PackInstall("part_size does not fit this platform's usize".into())
    })?;

    let mut parts = Vec::with_capacity(accounting.num_parts as usize);
    for (index, slice) in data.chunks(chunk).enumerate() {
        parts.push(PartEntry {
            index: index as u64,
            bytes: slice.len() as u64,
            sha256: sha256_hex(slice),
        });
    }

    Ok(MultiPartIndex {
        part_size,
        total_bytes: data.len() as u64,
        sha256: sha256_hex(data),
        parts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accounting_exact_multiple() {
        let a = part_accounting(10, 5).unwrap();
        assert_eq!(a.num_parts, 2);
        assert_eq!(a.last_part_bytes, 5);
        assert_eq!(a.part_bytes(0), Some(5));
        assert_eq!(a.part_bytes(1), Some(5));
        assert_eq!(a.part_bytes(2), None);
    }

    #[test]
    fn accounting_with_remainder() {
        let a = part_accounting(11, 5).unwrap();
        assert_eq!(a.num_parts, 3);
        assert_eq!(a.last_part_bytes, 1);
        assert_eq!(a.part_bytes(2), Some(1));
    }

    #[test]
    fn accounting_zero_total() {
        let a = part_accounting(0, 5).unwrap();
        assert_eq!(a.num_parts, 0);
        assert_eq!(a.last_part_bytes, 0);
        assert_eq!(a.part_bytes(0), None);
    }

    #[test]
    fn accounting_rejects_zero_part_size() {
        assert!(part_accounting(10, 0).is_err());
    }
}
