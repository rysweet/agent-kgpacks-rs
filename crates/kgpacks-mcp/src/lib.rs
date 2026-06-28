//! `kgpacks-mcp` — Model Context Protocol server exposing pack queries.
//!
//! Rust port of `@kgpacks/mcp`. The M1 scaffold models tool registration; the
//! real stdio MCP transport and tool schemas land in M5.

use kgpacks_db::GraphStore;
use kgpacks_packs::PackManifest;
use kgpacks_query::Retriever;

/// An MCP server advertising one query tool per registered pack.
#[derive(Debug, Default)]
pub struct McpServer {
    manifests: Vec<PackManifest>,
}

impl McpServer {
    /// Create an empty server.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a pack so its query tool is advertised.
    pub fn register(&mut self, manifest: PackManifest) {
        self.manifests.push(manifest);
    }

    /// Tool names advertised over MCP (`query.<pack_id>`).
    pub fn list_tools(&self) -> Vec<String> {
        self.manifests
            .iter()
            .map(|m| format!("query.{}", m.id()))
            .collect()
    }

    /// Open a fresh backing store (M1 placeholder for a LadybugDB connection).
    pub fn new_store() -> GraphStore {
        GraphStore::open_in_memory()
    }

    /// Handle an MCP query tool call by delegating to the retriever.
    pub fn handle_query(&self, retriever: &Retriever, question: &str) -> String {
        retriever.answer(question)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_and_lists_tools() {
        let mut server = McpServer::new();
        server.register(PackManifest::new("cve", "1.0.0"));
        assert_eq!(server.list_tools(), vec!["query.cve@1.0.0".to_string()]);
        assert_eq!(McpServer::new_store().node_count(), 0);
    }
}
