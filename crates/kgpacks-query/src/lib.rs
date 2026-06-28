//! `kgpacks-query` — hybrid retrieval (vector + FTS), reranking and cypher-RAG.
//!
//! Rust port of `@kgpacks/query`. The retriever owns a [`GraphStore`] and an
//! [`Embedder`], and synthesizes answers through an [`Agent`]. Real hybrid
//! retrieval, cross-encoder reranking and Cypher generation land in M4.

use kgpacks_agent::Agent;
use kgpacks_db::GraphStore;
use kgpacks_embeddings::Embedder;

/// Hybrid retriever combining graph, vector and full-text search, then an
/// optional graph-RAG synthesis step via the agent.
pub struct Retriever {
    store: GraphStore,
    embedder: Embedder,
    agent: Agent,
}

impl Retriever {
    /// Bind a retriever to a store, embedder and agent.
    pub fn new(store: GraphStore, embedder: Embedder, agent: Agent) -> Self {
        Self {
            store,
            embedder,
            agent,
        }
    }

    /// Borrow the underlying store.
    pub fn store(&self) -> &GraphStore {
        &self.store
    }

    /// Retrieve context and synthesize an answer (M1 placeholder pipeline).
    pub fn answer(&self, question: &str) -> String {
        let query_vec = self.embedder.embed(question);
        let context = format!("nodes={} dim={}", self.store.node_count(), query_vec.len());
        self.agent.answer(question, &context)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_over_empty_store() {
        let r = Retriever::new(
            GraphStore::open_in_memory(),
            Embedder::new(8),
            Agent::new("stub"),
        );
        let out = r.answer("what is x?");
        assert!(out.contains("nodes=0"));
    }
}
