//! Error type for the ingestion pipeline.

use thiserror::Error;

/// Errors raised by the ingestion pipeline.
#[derive(Error, Debug)]
pub enum IngestionError {
    /// A content source could not find the requested article (parity with the
    /// reference `ArticleNotFoundError`).
    #[error("article not found: {0}")]
    ArticleNotFound(String),

    /// A content source failed for a reason other than "not found".
    #[error("content source error: {0}")]
    ContentSource(String),

    /// An invalid work-queue state transition was requested (parity with the
    /// reference `ValueError("Invalid state: …")`).
    #[error("invalid state: {0}")]
    InvalidState(String),

    /// An error surfaced by the underlying graph store (`kgpacks-db`).
    #[error(transparent)]
    Db(#[from] kgpacks_db::Error),

    /// An error from batch embedding generation.
    #[error(transparent)]
    Embedding(#[from] kgpacks_embeddings::EmbeddingError),

    /// An I/O error while staging a bulk-load CSV for `COPY ENTITY_RELATION`.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Convenience result alias for fallible ingestion operations.
pub type Result<T> = std::result::Result<T, IngestionError>;
