//! `kgpacks-query` — retriever facade.
//!
//! Rust port of the CORE slice of `@kgpacks/query`'s `retriever.ts`. The single
//! public entry point for the read path: [`PackRetriever`] binds a connection
//! (plus an injected embedder and schema config) and exposes
//! [`retrieve`](PackRetriever::retrieve), dispatching to vector or hybrid
//! retrieval.
//!
//! Naming note: the M1 placeholder [`crate::Retriever`] (agent-grounded `answer`)
//! is retained for the not-yet-wired backend/mcp/eval/cli crates until M5 lands;
//! this real, pack-backed retriever is therefore named `PackRetriever`. The
//! ENHANCEMENTS stages (Cypher-RAG → reranker → cross-encoder → few-shot →
//! synthesis) and `retrieveAndSynthesize` are deferred to M5.

use std::cell::Cell;
use std::collections::HashSet;

use kgpacks_db::Connection;

use crate::constants::{
    default_stop_words, DEFAULT_K, DEFAULT_NODE_TABLE, DEFAULT_VECTOR_INDEX, DEFAULT_WEIGHTS,
};
use crate::errors::{QueryError, Result};
use crate::hybrid::hybrid_retrieve;
use crate::types::{Embedder, RetrieveMode, RetrieveOptions, RetrieverResult};
use crate::vector::{vector_retrieve, VectorConfig};

/// Construction options for [`PackRetriever::with_embedder`].
///
/// Mirrors the CORE fields of the TypeScript `CreateRetrieverOptions`
/// (`nodeTable` / `vectorIndex` / `stopWords`). [`Default`] reproduces the
/// reference defaults (`Section` / `embedding_idx` / the English stop-word set).
#[derive(Debug, Clone)]
pub struct RetrieverConfig {
    /// Node table holding the embeddings. Default `"Section"`.
    pub node_table: String,
    /// Vector index name over that table. Default `"embedding_idx"`.
    pub vector_index: String,
    /// Stop words for hybrid keyword extraction. Default English set.
    pub stop_words: HashSet<String>,
}

impl Default for RetrieverConfig {
    fn default() -> Self {
        Self {
            node_table: DEFAULT_NODE_TABLE.to_string(),
            vector_index: DEFAULT_VECTOR_INDEX.to_string(),
            stop_words: default_stop_words(),
        }
    }
}

/// A retriever bound to a [`Connection`], an embedder, and a pack schema.
///
/// Mirrors the object returned by the TypeScript `createRetriever`. Its
/// [`retrieve`](PackRetriever::retrieve) runs CORE vector/hybrid retrieval.
pub struct PackRetriever<'conn, 'db, E: Embedder> {
    conn: &'conn Connection<'db>,
    embedder: E,
    config: VectorConfig,
    stop_words: HashSet<String>,
    vector_loaded: Cell<bool>,
    fts_loaded: Cell<bool>,
}

impl<'conn, 'db> PackRetriever<'conn, 'db, kgpacks_embeddings::Embedder> {
    /// Bind a retriever to `conn` with the default BGE-parity embedder and the
    /// default pack schema (`Section` / `embedding_idx`).
    ///
    /// Mirrors `createRetriever(conn)` with all options defaulted.
    pub fn new(conn: &'conn Connection<'db>) -> Self {
        Self::with_embedder(
            conn,
            kgpacks_embeddings::Embedder::bge(),
            RetrieverConfig::default(),
        )
    }
}

impl<'conn, 'db, E: Embedder> PackRetriever<'conn, 'db, E> {
    /// Bind a retriever to `conn` with an explicit `embedder` and `config`.
    ///
    /// Mirrors `createRetriever(conn, { embedder, nodeTable, vectorIndex,
    /// stopWords })`. Injecting the embedder keeps retrieval deterministic in
    /// tests.
    pub fn with_embedder(
        conn: &'conn Connection<'db>,
        embedder: E,
        config: RetrieverConfig,
    ) -> Self {
        Self {
            conn,
            embedder,
            config: VectorConfig {
                node_table: config.node_table,
                vector_index: config.vector_index,
            },
            stop_words: config.stop_words,
            vector_loaded: Cell::new(false),
            fts_loaded: Cell::new(false),
        }
    }

    /// Lazily LOAD the LadybugDB extensions the read path needs, once per
    /// retriever: `vector` always; `fts` only when a hybrid query runs.
    ///
    /// Mirrors the TypeScript `ensureExtensions`: a fresh read connection must
    /// load the pack's vector/FTS extensions before `QUERY_VECTOR_INDEX` /
    /// keyword calls resolve. Loading is idempotent and cached so repeated
    /// retrievals don't re-issue it.
    fn ensure_extensions(&self, hybrid: bool) -> Result<()> {
        if !self.vector_loaded.get() {
            self.conn.load_extension("vector")?;
            self.vector_loaded.set(true);
        }
        if hybrid && !self.fts_loaded.get() {
            self.conn.load_extension("fts")?;
            self.fts_loaded.set(true);
        }
        Ok(())
    }

    /// Runs retrieval for `query`, returning at most `opts.k` ranked results.
    ///
    /// Mirrors the CORE behavior of the TypeScript `retrieve`: dispatch on
    /// `opts.mode` (default [`RetrieveMode::Vector`]) to vector or hybrid
    /// retrieval, after validating `k` (`>= 1`) and loading the needed
    /// extensions.
    pub fn retrieve(&self, query: &str, opts: &RetrieveOptions) -> Result<Vec<RetrieverResult>> {
        let k = opts.k.unwrap_or(DEFAULT_K);
        if k < 1 {
            return Err(QueryError::InvalidArgument(format!(
                "k must be a positive integer, got {k}"
            )));
        }
        if i64::try_from(k).is_err() {
            // The driver binds the result count as an INT64; reject a `k` that
            // would not fit rather than silently truncating it.
            return Err(QueryError::InvalidArgument(format!(
                "k is too large to bind as an INT64 limit, got {k}"
            )));
        }

        let hybrid = opts.mode == RetrieveMode::Hybrid;
        self.ensure_extensions(hybrid)?;

        if hybrid {
            let weights = opts.weights.unwrap_or(DEFAULT_WEIGHTS);
            hybrid_retrieve(
                self.conn,
                &self.embedder,
                query,
                k,
                &weights,
                &self.config,
                &self.stop_words,
            )
        } else {
            vector_retrieve(self.conn, &self.embedder, query, k, &self.config)
        }
    }
}
