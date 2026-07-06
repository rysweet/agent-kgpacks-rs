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

mod errors;
pub mod manifest;
pub mod pack;
pub mod signing;
pub mod versioning;

pub use errors::{PacksError, Result};

pub use manifest::{
    load_manifest, load_manifest_from_dir, manifest_path_in, pack_name_re, parse_manifest_str,
    save_manifest, validate_manifest, PackManifest, MANIFEST_FILENAME,
};

pub use signing::{
    decode_public_key, pack_release_signature_filename, parse_trusted_public_key, signature_plan,
    trusted_release_public_key, validate_signature_flags, verify_pack_index_signature,
    PackIndexSignature, SignatureInputs, SignaturePlan, SigningKeyPair, PUBLIC_KEY_LEN,
    SECRET_SEED_LEN, SIGNATURE_ALGORITHM, SIGNATURE_LEN,
};

pub use versioning::{
    compare_versions, is_valid_semver, latest_version, parse_version, sort_versions, ParsedVersion,
};

pub use pack::{
    build_pack, load_pack, Article, BuiltPack, Entity, LoadedPack, PackContent,
    GRAPH_STORE_FILENAME, NODE_TABLE_DDL, REL_TABLE_DDL, SCHEMA,
};
