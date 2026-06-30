//! `kgpacks-query` — retrieval over a LadybugDB knowledge pack.
//!
//! Rust port of `@kgpacks/query`. This crate ships the **M4 CORE retrieval
//! pipeline** — the read path that turns a natural-language query into a ranked
//! list of section hits over a built pack:
//!
//! * [`vector_retrieve`] — cosine vector search via `CALL QUERY_VECTOR_INDEX`
//!   (`vector.ts`).
//! * [`hybrid_retrieve`] — a weighted blend of three signals (`hybrid.ts`):
//!   vector cosine similarity, `LINKS_TO` graph proximity, and title-keyword
//!   (full-text) matches.
//! * [`PackRetriever`] — the facade binding a [`kgpacks_db::Connection`], an
//!   [`Embedder`], and a pack schema, dispatching `retrieve` to either mode
//!   (`retriever.ts`).
//! * [`validate_cypher`] — a standalone read-only Cypher allow-list
//!   (`cypher-safety.ts`).
//!
//! The agent-grounded graph-RAG query (`retrieveAndSynthesize`) lands here in
//! **M5**: [`retrieve_and_synthesize`] binds this retrieval read path to the
//! [`CopilotAgent`](kgpacks_agent::CopilotAgent) for grounded answer synthesis.
//! The broader ENHANCEMENTS layer (graph reranker, cross-encoder, few-shot,
//! Cypher-RAG, multi-document synthesis) remains a follow-up. The M1 placeholder
//! [`Retriever`] (agent-grounded `answer`) is retained in the [`legacy`] module
//! so the not-yet-wired backend/mcp/eval crates keep compiling.
//!
//! ## Schema contract
//!
//! Retrieval targets a single pack schema (config-driven, with the reference
//! defaults): a `node_table` (default `Section`) whose nodes carry `id`,
//! `title`, `content`, and an embedding column indexed by `vector_index`
//! (default `embedding_idx`), plus `LINKS_TO` edges between nodes for the graph
//! signal. The `id` may be `INT64` (the reference fixtures) or `STRING` (the RS
//! pack's `Section.id`); [`row::to_id_string`] handles both.

mod constants;
mod cypher_safety;
mod errors;
mod hybrid;
mod legacy;
mod retriever;
pub mod row;
mod synthesis;
mod types;
mod vector;

// ── M4 CORE retrieval surface ──────────────────────────────────────────────

pub use constants::{
    default_stop_words, DEFAULT_K, DEFAULT_NODE_TABLE, DEFAULT_VECTOR_INDEX, DEFAULT_WEIGHTS,
};
pub use cypher_safety::validate_cypher;
pub use errors::{CypherValidationError, QueryError, Result};
pub use hybrid::hybrid_retrieve;
pub use retriever::{PackRetriever, RetrieverConfig};
pub use types::{Embedder, HybridWeights, RetrieveMode, RetrieveOptions, RetrieverResult};
pub use vector::{run_vector_search, vector_retrieve, ScoredNode, VectorConfig};

// ── M5 graph-RAG query surface ─────────────────────────────────────────────

pub use synthesis::{retrieve_and_synthesize, GraphRagAnswer};

// ── M1 placeholder (retained for the not-yet-wired sibling crates) ──────────

pub use legacy::Retriever;
