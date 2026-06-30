//! `kgpacks-ingestion` — fetch, chunk, extract, embed and load into the store.
//!
//! Rust port of `@kgpacks/ingestion` (reference `bootstrap/src/{embeddings,
//! extraction,expansion,database}` + `bootstrap/schema`). M3 implements the
//! build-pack → ingest pipeline over the M2 [`kgpacks_db`] graph store:
//!
//! * [`schema`] — the working-store schema (`ryugraph_schema.py`).
//! * [`content`] — pluggable content sources (`sources/base.py`).
//! * [`extraction`] — LLM extraction schema, sanitization and normalization
//!   (`extraction/llm_extractor.py`), gated behind the [`Extractor`] trait.
//! * [`link_discovery`] — graph expansion (`expansion/link_discovery.py`).
//! * [`work_queue`] — the claim/heartbeat/reclaim state machine
//!   (`expansion/work_queue.py`).
//! * [`processor`] — the per-article fetch→embed→load step
//!   (`expansion/processor.py` + `database/loader.py`).
//! * [`orchestrator`] — the [`process_one`] step and the [`Orchestrator`]
//!   (`expansion/orchestrator.py`).
//!
//! Chunking and embedding generation live in [`kgpacks_embeddings`]
//! (`embeddings/{chunker,generator}.py`). Embeddings are generated and stored;
//! the HNSW vector index over them is hybrid retrieval (M4).

pub mod content;
pub mod error;
pub mod extraction;
pub mod link_discovery;
pub mod orchestrator;
pub mod processor;
pub mod schema;
pub mod work_queue;

mod util;

pub use content::{Article, ContentSource, MapContentSource, ParsedSection};
pub use error::{IngestionError, Result};
pub use extraction::{
    build_extraction_prompt, detect_domain, normalize_relation, parse_extraction_response,
    sanitize_entities, sanitize_key_facts, sanitize_relationships, Entity, ExtractionResult,
    Extractor, JsonExtractor, MockExtractor, Relationship, STANDARD_RELATIONS,
};
pub use link_discovery::LinkDiscovery;
pub use orchestrator::{
    process_one, ArticleInfo, ExpansionConfig, LinkDiscoverer, Orchestrator, ProcessOutcome,
    Processor, WorkQueue,
};
pub use processor::{sanitize_error, ArticleProcessor};
pub use schema::{
    apply_ingestion_schema, apply_ingestion_schema_with_dim, ingestion_schema_ddl,
    DEFAULT_EMBEDDING_DIM,
};
pub use work_queue::{ClaimedArticle, QueueStats, WorkQueueManager, VALID_STATES};

use kgpacks_agent::Agent;
use kgpacks_db::GraphStore;
use kgpacks_embeddings::Embedder;

/// A minimal document ingestion facade retained for the M1 `kgpacks demo` wiring.
///
/// **Placeholder.** This is the M1 fixed-window chunker over the deprecated
/// in-memory [`GraphStore`], kept so `kgpacks-cli`'s `demo` keeps compiling. The
/// real M3 pipeline is [`Orchestrator`] / [`ArticleProcessor`] over the
/// LadybugDB-backed [`kgpacks_db::Database`]; the CLI is wired to it in M5.
pub struct Ingestor {
    embedder: Embedder,
}

impl Ingestor {
    /// Build an ingestor that embeds chunks with `embedder`.
    pub fn new(embedder: Embedder) -> Self {
        Self { embedder }
    }

    /// Split `text` into fixed-size byte windows (M1 placeholder chunker).
    pub fn chunk(&self, text: &str, size: usize) -> Vec<String> {
        if size == 0 {
            return vec![text.to_string()];
        }
        text.as_bytes()
            .chunks(size)
            .map(|c| String::from_utf8_lossy(c).into_owned())
            .collect()
    }

    /// Chunk, embed and load `text` into `store`, returning the chunk count.
    pub fn ingest(&self, store: &mut GraphStore, text: &str) -> usize {
        let chunks = self.chunk(text, 256);
        for c in &chunks {
            let _vector = self.embedder.embed(c);
            store.add_node();
        }
        chunks.len()
    }

    /// Agent-driven query expansion (M1 placeholder).
    pub fn expand_query(&self, agent: &Agent, query: &str) -> String {
        agent.answer(query, "expansion")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_and_ingests() {
        let ing = Ingestor::new(Embedder::new(4));
        assert_eq!(ing.chunk("abcdef", 2).len(), 3);
        let mut store = GraphStore::open_in_memory();
        assert_eq!(ing.ingest(&mut store, "hello world"), 1);
        assert_eq!(store.node_count(), 1);
    }
}
