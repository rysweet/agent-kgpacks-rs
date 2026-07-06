//! Typed error hierarchy for `kgpacks-packs`.
//!
//! Ports `packages/packs/src/errors.ts`. All failures share a common
//! [`PacksError`] so callers can match the whole family or discriminate by
//! variant. Validation never fails silently and never leaves partial state.

use thiserror::Error;

/// The error family for knowledge-pack manifest, versioning and build/load
/// operations.
#[derive(Error, Debug)]
pub enum PacksError {
    /// A manifest (or version string) failed schema validation.
    #[error("manifest validation error: {0}")]
    ManifestValidation(String),

    /// Building or loading a pack over the graph store failed.
    #[error("pack install error: {0}")]
    PackInstall(String),

    /// A named pack could not be found.
    #[error("pack not found: {0}")]
    PackNotFound(String),

    /// An underlying graph-store (`kgpacks-db`) error.
    #[error(transparent)]
    Db(#[from] kgpacks_db::Error),

    /// An I/O error reading or writing a pack on disk.
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// A release-index signing or signature-verification failure (WS7).
    #[error("signature error: {0}")]
    Signature(String),
}

/// Convenience result alias for fallible `kgpacks-packs` operations.
pub type Result<T> = std::result::Result<T, PacksError>;
