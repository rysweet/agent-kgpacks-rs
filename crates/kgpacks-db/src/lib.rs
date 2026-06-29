//! `kgpacks-db` — graph + vector + full-text store.
//!
//! Rust port of the TypeScript `@kgpacks/db` package. The production storage
//! engine is LadybugDB (the `lbug` crate).
//!
//! M2 wires the real graph store: [`Database`] / [`Connection`] /
//! [`DatabaseOptions`] mirror `packages/db/src/database.ts` (open/close,
//! bound-parameter Cypher, extension loading). Vector/FTS indexing lands in M4.
//!
//! The legacy [`GraphStore`] placeholder is retained so the not-yet-wired
//! sibling crates (`kgpacks-query`, `kgpacks-cli`, …) keep compiling until their
//! own milestones land; new code should use [`Database`].

mod database;
mod error;

pub use database::{Connection, Database, DatabaseOptions, Row};
pub use error::{Error, Result};

/// LadybugDB value type, re-exported for use in bound query parameters and rows.
pub use lbug::Value;

/// Handle to the knowledge-graph store backing nodes, edges, vectors and
/// full-text indexes.
///
/// **Deprecated placeholder.** This is the M1 in-memory stub kept only so the
/// sibling crates that have not yet been wired to LadybugDB continue to compile.
/// New code should use [`Database`].
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
