//! `kgpacks-packs` — knowledge-pack manifests, schema and build/load.
//!
//! Rust port of `@kgpacks/packs`. M2 delivers the **schema + build/load** half:
//!
//! * [`manifest`] — the `manifest.json` model and validation gate
//!   (`packages/packs/src/manifest.ts`).
//! * [`versioning`] — SemVer 2.0 helpers (`packages/packs/src/versioning.ts`).
//! * [`pack`] — the LadybugDB-backed pack graph schema plus
//!   [`build_pack`](pack::build_pack) / [`load_pack`](pack::load_pack), whose
//!   round-trip over the graph store is the M2 acceptance gate.
//!
//! The registry, tarball installer and version resolution surfaces land in a
//! later milestone.

pub mod checkpoint;
pub mod corpus;
pub mod cve_build;
mod errors;
pub mod manifest;
pub mod pack;
mod sha256;
pub mod versioning;

pub use errors::{PacksError, Result};

pub use manifest::{
    load_manifest, load_manifest_from_dir, manifest_path_in, pack_name_re, parse_manifest_str,
    save_manifest, validate_manifest, PackManifest, MANIFEST_FILENAME,
};

pub use versioning::{
    compare_versions, is_valid_semver, latest_version, parse_version, sort_versions, ParsedVersion,
};

pub use pack::{
    build_pack, load_pack, Article, BuiltPack, Entity, LoadedPack, PackContent,
    GRAPH_STORE_FILENAME, NODE_TABLE_DDL, REL_TABLE_DDL, SCHEMA,
};

pub use checkpoint::{checkpoint_path_for, BuildCheckpoint, BuildCounts, CHECKPOINT_SUFFIX};

pub use corpus::{CorpusSource, CveEntity, CveRecord, CveRelation, FixtureCorpus};

pub use cve_build::{
    build_cve_pack, cve_schema_ddl, BuildParams, CveBuildReport, PipelineOptions, CVE_CATEGORY,
    DEFAULT_BATCH_SIZE, DEFAULT_QUEUE_CAPACITY,
};

/// A stable content fingerprint (SHA-256 hex) of `bytes`.
///
/// Callers building from a mutable source (e.g. a corpus file) fold this into
/// [`BuildParams::src`] so the build's `params_hash` binds the corpus *content*,
/// not just its path: if the content changes between an interrupted run and a
/// resume, the hash no longer matches and the build cleanly restarts instead of
/// mixing old and new records.
pub fn content_fingerprint(bytes: &[u8]) -> String {
    sha256::hex_digest(bytes)
}
