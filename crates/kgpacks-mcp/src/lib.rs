//! `kgpacks-mcp` — Model Context Protocol server exposing pack queries.
//!
//! Rust port of `@kgpacks/mcp`. The M1 scaffold models tool registration; the
//! real stdio MCP transport and tool schemas land in M5.
//!
//! The server resolves the directory that holds installed packs through the
//! SAME shared resolver as the CLI ([`kgpacks_packs::resolve_packs_dir`]), so a
//! pack installed by the CLI is found by the MCP server and vice versa. Callers
//! may override the location (e.g. for tests or embedding) via
//! [`McpServer::with_packs_dir`].

use std::path::{Path, PathBuf};

use kgpacks_db::GraphStore;
use kgpacks_packs::PackManifest;
use kgpacks_query::Retriever;

/// An MCP server advertising one query tool per registered pack.
#[derive(Debug)]
pub struct McpServer {
    manifests: Vec<PackManifest>,
    packs_dir: PathBuf,
}

impl Default for McpServer {
    fn default() -> Self {
        Self::new()
    }
}

impl McpServer {
    /// Create an empty server whose packs directory is the shared default
    /// (`--packs-dir` is not applicable here, so this resolves the
    /// `KGPACKS_PACKS_DIR` env var, else the XDG default — the SAME default the
    /// CLI uses).
    pub fn new() -> Self {
        Self {
            manifests: Vec::new(),
            packs_dir: kgpacks_packs::resolve_packs_dir(None),
        }
    }

    /// Create a server with an explicit packs directory override, mirroring the
    /// CLI's `--packs-dir` flag. Blank (empty/whitespace-only) overrides fall
    /// through to the env var / XDG default.
    pub fn with_packs_dir(dir: impl AsRef<str>) -> Self {
        Self {
            manifests: Vec::new(),
            packs_dir: kgpacks_packs::resolve_packs_dir(Some(dir.as_ref())),
        }
    }

    /// The directory this server reads installed packs from.
    pub fn packs_dir(&self) -> &Path {
        &self.packs_dir
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

    #[test]
    fn default_packs_dir_matches_the_shared_resolver() {
        // The MCP server MUST resolve the same default the CLI does, so a pack
        // installed by one is found by the other.
        let server = McpServer::new();
        assert_eq!(server.packs_dir(), kgpacks_packs::resolve_packs_dir(None));
    }

    #[test]
    fn explicit_override_is_honored() {
        let server = McpServer::with_packs_dir("/tmp/custom-packs");
        assert_eq!(server.packs_dir(), Path::new("/tmp/custom-packs"));
    }

    #[test]
    fn blank_override_falls_through_to_the_default() {
        // A whitespace-only override is treated as unset, so it resolves to the
        // same default as `new()`.
        let server = McpServer::with_packs_dir("   ");
        assert_eq!(server.packs_dir(), kgpacks_packs::resolve_packs_dir(None));
    }
}
