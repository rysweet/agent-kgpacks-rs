//! `kgpacks-query` — locked retrieval constants.
//!
//! Rust port of `@kgpacks/query`'s `constants.ts`, itself ported verbatim from
//! the reference (`rysweet/agent-kgpacks`) so the read path reproduces its
//! behavior. Overridable knobs (weights, top-k) are exposed publicly; values
//! fixed by parity (schema names, the per-signal multipliers, the stop-word set)
//! are crate-internal `pub(crate)` so they cannot drift via the public surface.

use crate::types::HybridWeights;

/// Default top-k for a retrieval call (reference `top_k`/`max_results` default).
pub const DEFAULT_K: usize = 10;

/// Default hybrid signal weights — reference `hybrid_retrieve` defaults
/// (`vector_weight=0.5, graph_weight=0.3, keyword_weight=0.2`).
pub const DEFAULT_WEIGHTS: HybridWeights = HybridWeights {
    vector: 0.5,
    graph: 0.3,
    keyword: 0.2,
};

/// Node table searched by the vector index (reference schema `Section`).
pub const DEFAULT_NODE_TABLE: &str = "Section";

/// Vector index name (reference schema `embedding_idx`).
pub const DEFAULT_VECTOR_INDEX: &str = "embedding_idx";

/// Per-match graph proximity contribution multiplier. Each `LINKS_TO` neighbor
/// of a seed node adds `graph_weight * GRAPH_MATCH` (reference
/// `graph_weight * 0.5`).
pub(crate) const GRAPH_MATCH: f64 = 0.5;

/// Per-match keyword contribution multiplier. A title `CONTAINS` hit adds
/// `keyword_weight * KEYWORD_MATCH` (reference `keyword_weight * 0.7`).
pub(crate) const KEYWORD_MATCH: f64 = 0.7;

/// Default similarity used when a vector hit lacks a usable distance (reference
/// `.get(..., 0.5)`).
pub(crate) const DEFAULT_SIMILARITY: f64 = 0.5;

/// Number of top scored nodes used as graph-traversal seeds (reference `[:3]`).
pub(crate) const MAX_GRAPH_SEEDS: usize = 3;

/// Number of leading keywords used for the keyword signal (reference `[:3]`).
pub(crate) const MAX_KEYWORDS: usize = 3;

/// Minimum token length for a keyword (reference `len(w) > 3`): a keyword must
/// be strictly longer than this.
pub(crate) const MIN_KEYWORD_LENGTH: usize = 3;

/// Read-only Cypher allow-list prefixes. A validated query (upper-cased, with
/// string literals stripped) must start with one of these (reference
/// `upper.startswith("MATCH") or upper.startswith("CALL")`).
pub(crate) const CYPHER_ALLOWED_PREFIXES: [&str; 2] = ["MATCH", "CALL"];

/// Blocked write/DDL keywords (reference `_CYPHER_BLOCKED_OPS`). Any occurrence
/// as a bare token outside a string literal rejects the query.
pub(crate) const CYPHER_BLOCKED_OPS: [&str; 7] = [
    "CREATE", "DELETE", "DROP", "SET", "MERGE", "REMOVE", "DETACH",
];

/// English stop words for keyword extraction — ported verbatim from the
/// reference `KnowledgeGraphAgent.STOP_WORDS` frozenset so keyword selection
/// matches. Exposed via [`default_stop_words`].
pub(crate) const DEFAULT_STOP_WORDS: &[&str] = &[
    "a", "an", "the", "and", "or", "but", "in", "on", "at", "to", "for", "of", "with", "by",
    "from", "is", "are", "was", "were", "be", "been", "being", "have", "has", "had", "do", "does",
    "did", "will", "would", "could", "should", "may", "might", "shall", "can", "not", "no", "nor",
    "so", "yet", "both", "either", "neither", "as", "if", "then", "than", "that", "this", "these",
    "those", "it", "its", "i", "we", "you", "he", "she", "they", "me", "us", "him", "her", "them",
    "my", "our", "your", "his", "their", "what", "which", "who", "whom", "when", "where", "why",
    "how", "all", "any", "each", "few", "more", "most", "other", "some", "such",
];

/// The default English stop-word set used for hybrid keyword extraction.
///
/// Mirrors the TypeScript `DEFAULT_STOP_WORDS` export. Returns a fresh owned set
/// so callers may extend or replace it when constructing a retriever.
pub fn default_stop_words() -> std::collections::HashSet<String> {
    DEFAULT_STOP_WORDS
        .iter()
        .map(|w| (*w).to_string())
        .collect()
}
