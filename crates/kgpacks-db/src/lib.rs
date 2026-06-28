//! `kgpacks-db` — graph + vector + full-text store.
//!
//! Rust port of the TypeScript `@kgpacks/db` package. The production storage
//! engine is LadybugDB (the `lbug` crate); this M1 scaffold ships an in-memory
//! placeholder and the real LadybugDB wiring (graph + schema parity) lands in
//! M2, with vector/FTS indexing in M4.

/// Handle to the knowledge-graph store backing nodes, edges, vectors and
/// full-text indexes.
#[derive(Debug, Default)]
pub struct GraphStore {
    nodes: usize,
}

impl GraphStore {
    /// Open an empty in-memory store (M1 placeholder for a LadybugDB instance).
    pub fn open_in_memory() -> Self {
        Self::default()
    }

    /// Insert a node, returning the new node count.
    pub fn add_node(&mut self) -> usize {
        self.nodes += 1;
        self.nodes
    }

    /// Number of nodes currently stored.
    pub fn node_count(&self) -> usize {
        self.nodes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_empty_and_inserts() {
        let mut store = GraphStore::open_in_memory();
        assert_eq!(store.node_count(), 0);
        store.add_node();
        assert_eq!(store.node_count(), 1);
    }
}
