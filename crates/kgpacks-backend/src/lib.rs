//! `kgpacks-backend` — HTTP API surface.
//!
//! Rust port of `@kgpacks/backend`. The M1 scaffold wires the retrieval stack
//! behind a single `query` handler; the real HTTP server, SSE streaming and
//! rate limiting land in M5.

use kgpacks_agent::Agent;
use kgpacks_db::GraphStore;
use kgpacks_embeddings::Embedder;
use kgpacks_query::Retriever;

/// The backend application state.
pub struct Backend {
    retriever: Retriever,
}

impl Backend {
    /// Build a backend around an existing retriever.
    pub fn new(retriever: Retriever) -> Self {
        Self { retriever }
    }

    /// Bootstrap a backend with a default in-memory stack (M1 placeholder).
    pub fn bootstrap() -> Self {
        let store = GraphStore::open_in_memory();
        let retriever = Retriever::new(store, Embedder::new(384), Agent::new("copilot-stub"));
        Self::new(retriever)
    }

    /// Handle a `POST /query` request body, returning the answer.
    pub fn query(&self, question: &str) -> String {
        self.retriever.answer(question)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstraps_and_answers() {
        let backend = Backend::bootstrap();
        assert!(backend.query("ping").contains("nodes="));
    }
}
