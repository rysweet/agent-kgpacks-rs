//! `kgpacks-query` — M1 placeholder retriever (agent-grounded `answer`).
//!
//! This is the original M1 scaffold [`Retriever`]: an in-memory
//! [`GraphStore`]-backed handle that embeds a question and asks an [`Agent`] to
//! synthesize an answer over a templated context. It is retained unchanged so
//! the not-yet-wired sibling crates (`kgpacks-backend`, `kgpacks-mcp`,
//! `kgpacks-eval`, `kgpacks-cli`) keep compiling until M5 wires the real
//! Copilot-SDK agent and graph-RAG pipeline.
//!
//! New retrieval code should use the M4 [`crate::PackRetriever`] (real
//! vector/hybrid retrieval over a LadybugDB pack).

use kgpacks_agent::Agent;
use kgpacks_db::GraphStore;
use kgpacks_embeddings::Embedder;

/// Placeholder graph-RAG retriever combining an in-memory store, an embedder,
/// and an agent (M1 scaffold; superseded for real retrieval by
/// [`crate::PackRetriever`]).
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
