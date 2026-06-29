//! Error type for the `kgpacks-db` graph store.

use thiserror::Error;

/// Errors raised by the [`crate::Database`] / [`crate::Connection`] wrapper.
///
/// Mirrors the failure modes of the TypeScript `@kgpacks/db` surface: operating
/// on a closed handle, and any error bubbling up from the underlying LadybugDB
/// (`lbug`) engine (query/prepare failures, invalid Cypher, I/O, …).
#[derive(Error, Debug)]
pub enum Error {
    /// A method was called on a [`crate::Database`] whose `close()` already ran.
    #[error("database is closed")]
    DatabaseClosed,

    /// A method was called on a [`crate::Connection`] whose `close()` already ran.
    #[error("connection is closed")]
    ConnectionClosed,

    /// An error surfaced by the underlying LadybugDB (`lbug`) engine.
    #[error(transparent)]
    Lbug(#[from] lbug::Error),
}

/// Convenience result alias for fallible `kgpacks-db` operations.
pub type Result<T> = std::result::Result<T, Error>;
