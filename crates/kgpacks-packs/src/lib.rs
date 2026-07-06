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
//! * [`release`] — the multi-part release index (split/accounting) used to
//!   publish and re-verify packs larger than
//!   [`MAX_SINGLE_ARTIFACT_BYTES`](release::MAX_SINGLE_ARTIFACT_BYTES).
//! * [`sha256`] — a self-contained SHA-256 (no external crypto dependency)
//!   backing the release index's per-part and overall content hashes.
//!
//! The registry, tarball installer and version resolution surfaces land in a
//! later milestone.

mod errors;
pub mod manifest;
pub mod pack;
pub mod release;
pub mod sha256;
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
    build_pack, load_pack, plan_load_statements, Article, BuiltPack, Entity, LoadedPack,
    PackContent, PlannedStatement, CREATE_HAS_ENTITY_CYPHER, GRAPH_STORE_FILENAME, NODE_TABLE_DDL,
    REL_TABLE_DDL, SCHEMA,
};

pub use release::{
    part_accounting, plan_multipart_release, requires_multipart, MultiPartIndex, PartAccounting,
    PartEntry, MAX_SINGLE_ARTIFACT_BYTES,
};

pub use sha256::{sha256_hex, Sha256};
