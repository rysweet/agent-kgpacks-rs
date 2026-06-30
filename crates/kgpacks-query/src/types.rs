//! `kgpacks-query` — public types (the retrieval contract surface).
//!
//! Rust port of the CORE slice of `@kgpacks/query`'s `types.ts`. Kept free of
//! engine details so consumers can depend on the shapes without pulling in the
//! database or embeddings internals. The ENHANCEMENTS contracts (reranker,
//! cross-encoder, few-shot, Cypher-RAG, synthesis) are deferred to M5.

use crate::errors::Result;

/// A single ranked retrieval hit: a node id, its score, and its section text.
///
/// Mirrors the TypeScript `RetrieverResult`.
#[derive(Debug, Clone, PartialEq)]
pub struct RetrieverResult {
    /// Stable string form of the source node's primary key.
    pub id: String,
    /// Relevance score, higher is better. For [`RetrieveMode::Vector`] this is
    /// cosine similarity `1 - distance` clamped to `[0, 1]`; for
    /// [`RetrieveMode::Hybrid`] it is the weighted sum of the vector, graph, and
    /// keyword signals.
    pub score: f64,
    /// The retrieved section content.
    pub content: String,
}

/// Retrieval strategy. Mirrors the TypeScript `RetrieveMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RetrieveMode {
    /// Cosine vector search only (the default).
    #[default]
    Vector,
    /// Weighted blend of the vector, graph-proximity, and keyword signals.
    Hybrid,
}

/// Per-signal weights for [`RetrieveMode::Hybrid`]. Mirrors the TypeScript
/// `HybridWeights`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HybridWeights {
    /// Weight applied to the cosine-similarity signal.
    pub vector: f64,
    /// Weight applied to the graph-proximity (`LINKS_TO`) signal.
    pub graph: f64,
    /// Weight applied to the title keyword-match signal.
    pub keyword: f64,
}

/// Options for a single [`crate::PackRetriever::retrieve`] call.
///
/// Mirrors the CORE fields of the TypeScript `RetrieveOptions`
/// (`k` / `mode` / `weights`). `None` fields fall back to the locked defaults
/// (`DEFAULT_K`, [`RetrieveMode::Vector`], `DEFAULT_WEIGHTS`).
#[derive(Debug, Clone, Default)]
pub struct RetrieveOptions {
    /// Number of results to return (top-k). Defaults to `DEFAULT_K` (10).
    pub k: Option<usize>,
    /// Retrieval strategy. Defaults to [`RetrieveMode::Vector`].
    pub mode: RetrieveMode,
    /// Hybrid signal weights. Defaults to `DEFAULT_WEIGHTS`.
    pub weights: Option<HybridWeights>,
}

/// Minimal structural contract for a query embedder.
///
/// Mirrors the TypeScript `Embedder` interface (`generateQuery`). Accepting the
/// trait rather than the concrete [`kgpacks_embeddings::Embedder`] keeps the
/// retriever injectable for deterministic tests (which supply a fixed embedder
/// so exact cosine arithmetic is known). The crate provides a blanket-free impl
/// for [`kgpacks_embeddings::Embedder`].
pub trait Embedder {
    /// Embeds search queries, returning one vector per input query (BGE-family
    /// embedders apply their query-instruction prefix).
    fn generate_query(&self, queries: &[&str]) -> Result<Vec<Vec<f32>>>;
}

impl Embedder for kgpacks_embeddings::Embedder {
    fn generate_query(&self, queries: &[&str]) -> Result<Vec<Vec<f32>>> {
        kgpacks_embeddings::Embedder::generate_query(self, queries)
            .map_err(|e| crate::errors::QueryError::Embedding(e.to_string()))
    }
}
