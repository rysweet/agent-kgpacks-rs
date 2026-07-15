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
//! * [`release`] — the pack-release model covering both the versioned
//!   release-tag/version/provenance derivation (`scripts/release-pack.mjs` +
//!   `packVersionFromReleaseTag`): dated tags `<name>-YYYY.MM[.N]` derived to
//!   unpadded SemVer, the stable `packs` latest-pointer, and the
//!   `<name>.pack-release.json` provenance mirror; and the multi-part release
//!   index (split/accounting) used to publish and re-verify packs larger than
//!   [`MAX_SINGLE_ARTIFACT_BYTES`](release::MAX_SINGLE_ARTIFACT_BYTES).
//! * [`sha256`] — a self-contained SHA-256 (no external crypto dependency)
//!   backing the release index's per-part and overall content hashes.
//! * [`registry`] — read-path queries over an install root
//!   ([`list_packs`](registry::list_packs)), backing the CLI `status` command
//!   (`packages/packs/src/registry.ts`).
//! * [`cve_build`] — a resumable, pipelined CVE pack builder
//!   ([`build_cve_pack`](cve_build::build_cve_pack)): checkpoint/resume via the
//!   [`checkpoint`] sidecar and a bounded `embed || load` pipeline, consuming the
//!   [`corpus`] ([`CorpusSource`](corpus::CorpusSource)) seam.
//!
//! The tarball installer and byte-level release packaging land in a later
//! milestone.

pub mod checkpoint;
pub mod corpus;
pub mod cve_build;
mod errors;
pub mod manifest;
pub mod pack;
pub mod packs_dir;
pub mod registry;
pub mod release;
pub mod sha256;
pub mod signing;
pub mod versioning;

pub use errors::{PacksError, Result};

pub use packs_dir::{ensure_packs_dir, resolve_packs_dir, PACKS_DIR_ENV};

pub use manifest::{
    load_manifest, load_manifest_from_dir, manifest_path_in, pack_name_re, parse_manifest_str,
    save_manifest, validate_manifest, PackManifest, MANIFEST_FILENAME,
};

pub use signing::{
    decode_public_key, decode_secret_seed, pack_release_signature_filename,
    parse_trusted_public_key, signature_plan, trusted_release_public_key, validate_signature_flags,
    verify_pack_index_signature, PackIndexSignature, SignatureInputs, SignaturePlan,
    SigningKeyPair, PUBLIC_KEY_LEN, SECRET_SEED_LEN, SIGNATURE_ALGORITHM, SIGNATURE_LEN,
};

pub use versioning::{
    compare_versions, is_valid_semver, latest_version, pack_version_from_release_tag,
    parse_version, sort_versions, ParsedVersion,
};

pub use release::{
    build_release_index, build_release_provenance, iso8601_utc_from_unix, latest_release_tag,
    now_iso8601_utc, pack_part_filename, pack_release_filename, plan_release, publish_targets,
    resolve_model, resolve_release_version, PackReleaseIndex, ProvenanceOverrides, ReleasePart,
    ReleasePlan, LATEST_POINTER_TAG, RELEASE_INDEX_FORMAT,
};

pub use pack::{
    build_pack, load_pack, plan_load_statements, Article, BuiltPack, Entity, LoadedPack,
    PackContent, PlannedStatement, CREATE_HAS_ENTITY_CYPHER, GRAPH_STORE_FILENAME, NODE_TABLE_DDL,
    REL_TABLE_DDL, SCHEMA,
};

pub use release::{
    part_accounting, plan_multipart_release, requires_multipart, MultiPartIndex, PartAccounting,
    PartEntry, MAX_SINGLE_ARTIFACT_BYTES,
};

pub use sha256::{sha256_hex, Sha256};

pub use registry::{list_packs, InstalledPack};

pub use checkpoint::{checkpoint_path_for, BuildCheckpoint, BuildCounts, CHECKPOINT_SUFFIX};

pub use corpus::{CorpusSource, CveEntity, CveRecord, CveRelation, FixtureCorpus};

pub use cve_build::{
    build_cve_pack, cve_schema_ddl, BuildParams, CveBuildReport, PipelineOptions,
    CREATE_ENTITY_RELATION_CYPHER, CVE_CATEGORY, DEFAULT_BATCH_SIZE, DEFAULT_QUEUE_CAPACITY,
};

/// A stable content fingerprint (SHA-256 hex) of `bytes`.
///
/// Callers building from a mutable source (e.g. a corpus file) fold this into
/// [`BuildParams::src`] so the build's `params_hash` binds the corpus *content*,
/// not just its path: if the content changes between an interrupted run and a
/// resume, the hash no longer matches and the build cleanly restarts instead of
/// mixing old and new records.
pub fn content_fingerprint(bytes: &[u8]) -> String {
    sha256::sha256_hex(bytes)
}
