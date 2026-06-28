//! `kgpacks-cli` — command-line interface.
//!
//! Rust port of `@kgpacks/cli`. The M1 scaffold provides a placeholder command
//! dispatch and a `demo` subcommand that wires one piece of every crate
//! together to prove the pipeline shape compiles. Real `clap`-based subcommands
//! (ingest / build / query / eval / serve / mcp) land across M2-M5.

use kgpacks_agent::Agent;
use kgpacks_db::GraphStore;
use kgpacks_embeddings::Embedder;
use kgpacks_eval::{EvalCase, Harness};
use kgpacks_ingestion::Ingestor;
use kgpacks_packs::PackManifest;
use kgpacks_query::Retriever;

/// Run the CLI with the given argument list (excluding the program name).
pub fn run(args: &[String]) -> Result<String, String> {
    let cmd = args.first().map(String::as_str).unwrap_or("help");
    match cmd {
        "help" | "--help" | "-h" => Ok(help_text()),
        "version" | "--version" | "-V" => Ok(env!("CARGO_PKG_VERSION").to_string()),
        "demo" => Ok(demo()),
        other => Err(format!("unknown command: {other}")),
    }
}

fn help_text() -> String {
    "kgpacks <command>\n  commands: ingest  build  query  eval  serve  mcp  demo  version"
        .to_string()
}

/// Wire the full pack -> ingest -> retrieve -> eval flow end to end (stubs).
fn demo() -> String {
    let mut store = GraphStore::open_in_memory();
    let embedder = Embedder::new(384);
    let ingestor = Ingestor::new(embedder.clone());
    let chunks = ingestor.ingest(&mut store, "hello world");
    let retriever = Retriever::new(store, embedder, Agent::new("copilot-stub"));
    let harness = Harness::new(PackManifest::new("demo", "0.1.0"), Agent::new("judge"));
    let case = EvalCase {
        question: "what is in the pack?".to_string(),
        expected: "nodes".to_string(),
    };
    let score = harness.score(&retriever, &case);
    format!(
        "ingested {chunks} chunk(s); pack={}; score={score}",
        harness.pack_id()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_version() {
        let out = run(&["version".to_string()]).unwrap();
        assert_eq!(out, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn rejects_unknown_command() {
        assert!(run(&["bogus".to_string()]).is_err());
    }

    #[test]
    fn demo_wires_the_pipeline() {
        let out = run(&["demo".to_string()]).unwrap();
        assert!(out.contains("ingested 1 chunk(s)"));
        assert!(out.contains("score=1"));
    }
}
