//! `kgpacks-query` — error taxonomy.
//!
//! Rust port of `@kgpacks/query`'s `errors.ts`. The retrieval read path *fails
//! closed*: it returns valid results or one of these errors. [`QueryError`] is
//! the crate-wide error; [`CypherValidationError`] is the specific failure
//! [`crate::validate_cypher`] raises and converts into [`QueryError`] (mirroring
//! the TypeScript `CypherValidationError extends QueryError` relationship via a
//! `From` impl).

use thiserror::Error;

/// The specific error [`crate::validate_cypher`] raises when a query fails the
/// read-only allow-list (a non-`MATCH`/`CALL` prefix, a blocked write/DDL
/// keyword, or a variable-length path). The message names the precise reason so
/// rejections are auditable.
///
/// Mirrors the TypeScript `CypherValidationError`. It converts into
/// [`QueryError`] (the TS class hierarchy `CypherValidationError extends
/// QueryError`), so callers can treat every query failure uniformly as a
/// [`QueryError`] while still being able to match this specific case.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{0}")]
pub struct CypherValidationError(pub String);

/// The crate-wide error for the retrieval read path.
///
/// Mirrors the TypeScript `QueryError` base: every failure the package surfaces
/// is one of these variants.
#[derive(Debug, Error)]
pub enum QueryError {
    /// An invalid retrieval argument (e.g. a non-positive `k`). Mirrors the
    /// TypeScript `new QueryError('k must be a positive integer, …')`.
    #[error("{0}")]
    InvalidArgument(String),

    /// A Cypher safety-validation failure. Wraps [`CypherValidationError`] so the
    /// standalone validator's error participates in the unified taxonomy.
    #[error(transparent)]
    CypherValidation(#[from] CypherValidationError),

    /// An error bubbled up from the underlying graph store (`kgpacks-db`):
    /// driver/query failures, a closed handle, invalid Cypher, I/O, …
    #[error(transparent)]
    Db(#[from] kgpacks_db::Error),

    /// An embedding-generation failure surfaced by the injected embedder.
    #[error("embedding failed: {0}")]
    Embedding(String),
}

/// Convenience result alias for fallible `kgpacks-query` operations.
pub type Result<T> = std::result::Result<T, QueryError>;
