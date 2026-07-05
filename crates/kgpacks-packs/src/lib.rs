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
//! * [`registry`] — read-path queries over an install root
//!   ([`list_packs`](registry::list_packs)), backing the CLI `status` command
//!   (`packages/packs/src/registry.ts`).
//!
//! The tarball installer and version-resolution surfaces land in a later
//! milestone.

mod errors;
pub mod manifest;
pub mod pack;
pub mod registry;
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

pub use registry::{list_packs, InstalledPack};
