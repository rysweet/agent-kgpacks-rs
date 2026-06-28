//! `kgpacks-ingestion` — fetch, chunk, extract, embed and load into the store.
//!
//! Rust port of `@kgpacks/ingestion`. The M1 scaffold implements a fixed-window
//! chunker and loads placeholder nodes into a [`GraphStore`]; fetching,
//! extraction and graph expansion land in M3.

use kgpacks_agent::Agent;
use kgpacks_db::GraphStore;
use kgpacks_embeddings::Embedder;

/// A document ingestion pipeline.
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

    /// Agent-driven query expansion (M1 placeholder; real prompts land in M3).
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
