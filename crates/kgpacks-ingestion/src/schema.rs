//! The ingestion working-store schema.
//!
//! Rust port of `bootstrap/schema/ryugraph_schema.py`. This is the **working**
//! graph the expansion pipeline reads and writes (articles in flight through the
//! `discovered → claimed → loaded → processed` state machine), as distinct from
//! the cleaned, published *pack* schema in `kgpacks-packs`.
//!
//! Two deliberate differences from the reference, documented for parity:
//!
//! * **Timestamps as `INT64` epoch-milliseconds.** The reference uses `TIMESTAMP`
//!   for `claimed_at` / `processed_at`. Storing epoch-millis as `INT64` keeps the
//!   working store self-contained (no timezone parsing) and makes stale-claim
//!   comparisons a plain integer `<`, which is what [`crate::WorkQueueManager`]
//!   relies on. Integer columns are widened from `INT32` to `INT64` to match the
//!   `kgpacks-db` bound-parameter surface (`Value::Int64`).
//! * **Vector index deferred to M4.** Embedding columns (`DOUBLE[dim]`) are
//!   created and populated here (M3 generates and stores embeddings), but the
//!   `CREATE_VECTOR_INDEX` HNSW indexes and the `VECTOR`/`FTS` extensions are
//!   part of hybrid retrieval (M4) and are not loaded by the working store.

use kgpacks_db::Connection;

use crate::error::Result;

/// Default embedding dimensionality for the working store, matching the
/// reference 768-d model and [`kgpacks_embeddings::DEFAULT_DIM`].
pub const DEFAULT_EMBEDDING_DIM: usize = kgpacks_embeddings::DEFAULT_DIM;

/// Build the working-store schema DDL (node tables first, then relationship
/// tables) for an embedding column width of `embedding_dim`.
///
/// The order matters: every relationship table references node tables that must
/// already exist.
pub fn ingestion_schema_ddl(embedding_dim: usize) -> Vec<String> {
    vec![
        // ── Node tables ──────────────────────────────────────────────────
        "CREATE NODE TABLE Article(\
            title STRING, \
            category STRING, \
            word_count INT64, \
            expansion_state STRING, \
            expansion_depth INT64, \
            claimed_at INT64, \
            processed_at INT64, \
            retry_count INT64, \
            PRIMARY KEY(title))"
            .to_string(),
        format!(
            "CREATE NODE TABLE Section(\
                section_id STRING, \
                title STRING, \
                content STRING, \
                embedding DOUBLE[{embedding_dim}], \
                level INT64, \
                word_count INT64, \
                PRIMARY KEY(section_id))"
        ),
        "CREATE NODE TABLE Category(\
            name STRING, \
            article_count INT64, \
            PRIMARY KEY(name))"
            .to_string(),
        "CREATE NODE TABLE Entity(\
            entity_id STRING, \
            name STRING, \
            type STRING, \
            description STRING, \
            PRIMARY KEY(entity_id))"
            .to_string(),
        "CREATE NODE TABLE Fact(\
            fact_id STRING, \
            content STRING, \
            PRIMARY KEY(fact_id))"
            .to_string(),
        format!(
            "CREATE NODE TABLE Chunk(\
                chunk_id STRING, \
                content STRING, \
                embedding DOUBLE[{embedding_dim}], \
                article_title STRING, \
                section_index INT64, \
                chunk_index INT64, \
                PRIMARY KEY(chunk_id))"
        ),
        // ── Relationship tables ──────────────────────────────────────────
        "CREATE REL TABLE HAS_SECTION(FROM Article TO Section, section_index INT64)".to_string(),
        "CREATE REL TABLE LINKS_TO(FROM Article TO Article, link_type STRING)".to_string(),
        "CREATE REL TABLE IN_CATEGORY(FROM Article TO Category)".to_string(),
        "CREATE REL TABLE HAS_ENTITY(FROM Article TO Entity)".to_string(),
        "CREATE REL TABLE HAS_FACT(FROM Article TO Fact)".to_string(),
        "CREATE REL TABLE ENTITY_RELATION(FROM Entity TO Entity, relation STRING, context STRING)"
            .to_string(),
        "CREATE REL TABLE HAS_CHUNK(FROM Article TO Chunk, section_index INT64, chunk_index INT64)"
            .to_string(),
    ]
}

/// Apply the working-store schema to `conn` using [`DEFAULT_EMBEDDING_DIM`].
pub fn apply_ingestion_schema(conn: &Connection<'_>) -> Result<()> {
    apply_ingestion_schema_with_dim(conn, DEFAULT_EMBEDDING_DIM)
}

/// Apply the working-store schema to `conn` with an explicit `embedding_dim`
/// (the embedder used by the pipeline must produce vectors of this width).
pub fn apply_ingestion_schema_with_dim(conn: &Connection<'_>, embedding_dim: usize) -> Result<()> {
    for ddl in ingestion_schema_ddl(embedding_dim) {
        conn.run(&ddl)?;
    }
    Ok(())
}
