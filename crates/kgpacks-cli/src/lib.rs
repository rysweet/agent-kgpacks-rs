//! `kgpacks-cli` — command-line interface (`kgpacks` binary).
//!
//! Rust port of `@kgpacks/cli`. This milestone (M5) wires the **graph-RAG query
//! path** end to end over a built pack: the `query` command runs M4 graph +
//! vector + FTS retrieval and prints the ranked hits as JSON (mirroring the
//! reference `query` command), and the `ask` command runs the full graph-RAG
//! query — retrieval followed by grounded synthesis via the
//! [`CopilotAgent`](kgpacks_agent::CopilotAgent).
//!
//! The `ask` command's transport is injectable: production wires the real
//! Copilot transport (built with the `copilot` feature), while tests inject a
//! mock via [`run_with_transport`], so the end-to-end flow is proven offline.
//! The `demo` subcommand (M1) is retained as a cheap smoke test.

use std::path::{Path, PathBuf};

use kgpacks_agent::{Agent, CopilotAgent, CopilotAgentOptions, Transport};
use kgpacks_db::{Database, GraphStore};
use kgpacks_embeddings::Embedder;
use kgpacks_eval::{EvalCase, Harness};
use kgpacks_ingestion::Ingestor;
use kgpacks_packs::PackManifest;
use kgpacks_query::{
    retrieve_and_synthesize, PackRetriever, RetrieveMode, RetrieveOptions, Retriever, DEFAULT_K,
};

/// Filename of the LadybugDB graph store inside a pack directory (matches
/// `kgpacks_packs::GRAPH_STORE_FILENAME`).
const DB_FILENAME: &str = "graph.lbug";

/// Environment variable naming the directory that holds installed packs.
const PACKS_DIR_ENV: &str = "KGPACKS_PACKS_DIR";

/// Default packs directory, relative to the working directory.
const DEFAULT_PACKS_DIR: &str = "packs";

/// A factory for the graph-RAG agent's transport (the `ask` execution seam).
pub type TransportFactory<'a> = &'a dyn Fn() -> Box<dyn Transport>;

/// Run the CLI with the given argument list (excluding the program name).
///
/// Production entry point: the `ask` command uses the real Copilot transport
/// when built with the `copilot` feature, and otherwise reports that synthesis
/// requires it.
pub fn run(args: &[String]) -> Result<String, String> {
    #[cfg(feature = "copilot")]
    {
        let factory = || -> Box<dyn Transport> { Box::new(kgpacks_agent::copilot_transport()) };
        dispatch(args, Some(&factory))
    }
    #[cfg(not(feature = "copilot"))]
    {
        dispatch(args, None)
    }
}

/// Run the CLI with an injected transport factory for the `ask` command.
///
/// Used by the end-to-end tests to drive the full graph-RAG flow offline
/// against a mock transport.
pub fn run_with_transport(
    args: &[String],
    make_transport: TransportFactory<'_>,
) -> Result<String, String> {
    dispatch(args, Some(make_transport))
}

fn dispatch(
    args: &[String],
    make_transport: Option<TransportFactory<'_>>,
) -> Result<String, String> {
    let (packs_dir, rest) = extract_packs_dir(args);
    let cmd = rest.first().map(String::as_str).unwrap_or("help");
    match cmd {
        "help" | "--help" | "-h" => Ok(help_text()),
        "version" | "--version" | "-V" => Ok(env!("CARGO_PKG_VERSION").to_string()),
        "demo" => Ok(demo()),
        "query" => cmd_query(&packs_dir, &rest[1..]),
        "ask" => cmd_ask(&packs_dir, &rest[1..], make_transport),
        other => Err(format!("unknown command: {other}")),
    }
}

fn help_text() -> String {
    [
        "kgpacks <command> [--packs-dir <dir>]",
        "  query <pack> <question> [-k <n>] [--mode vector|hybrid]   ranked retrieval as JSON",
        "  ask   <pack> <question> [-k <n>] [--mode vector|hybrid]   graph-RAG answer as JSON",
        "  demo                                                      smoke-test the pipeline",
        "  version                                                   print the version",
    ]
    .join("\n")
}

/// Pull a global `--packs-dir <dir>` flag out of `args` (anywhere in the list),
/// returning the resolved packs directory and the remaining arguments.
///
/// Resolution precedence mirrors the reference: the flag wins, else the
/// `KGPACKS_PACKS_DIR` env var, else `./packs`.
fn extract_packs_dir(args: &[String]) -> (PathBuf, Vec<String>) {
    let mut flag: Option<String> = None;
    let mut rest: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--packs-dir" {
            if let Some(value) = args.get(i + 1) {
                flag = Some(value.clone());
                i += 2;
                continue;
            }
        }
        rest.push(args[i].clone());
        i += 1;
    }
    let dir = flag
        .or_else(|| std::env::var(PACKS_DIR_ENV).ok())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_PACKS_DIR));
    (dir, rest)
}

/// Parsed positional + option arguments shared by `query` and `ask`.
struct QueryArgs {
    pack: String,
    question: String,
    k: usize,
    mode: RetrieveMode,
}

fn parse_query_args(args: &[String]) -> Result<QueryArgs, String> {
    let mut positionals: Vec<String> = Vec::new();
    let mut k = DEFAULT_K;
    let mut mode = RetrieveMode::default();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-k" | "--k" => {
                let raw = args
                    .get(i + 1)
                    .ok_or_else(|| "missing value for -k".to_string())?;
                k = raw
                    .parse::<usize>()
                    .ok()
                    .filter(|&n| n >= 1)
                    .ok_or_else(|| format!("k must be a positive integer, got {raw}"))?;
                i += 2;
            }
            "--mode" => {
                let raw = args
                    .get(i + 1)
                    .ok_or_else(|| "missing value for --mode".to_string())?;
                mode = match raw.as_str() {
                    "vector" => RetrieveMode::Vector,
                    "hybrid" => RetrieveMode::Hybrid,
                    other => return Err(format!("unknown retrieval mode: {other}")),
                };
                i += 2;
            }
            other => {
                positionals.push(other.to_string());
                i += 1;
            }
        }
    }

    let mut it = positionals.into_iter();
    let pack = it
        .next()
        .ok_or_else(|| "missing <pack> argument".to_string())?;
    let question = it
        .next()
        .ok_or_else(|| "missing <question> argument".to_string())?;
    Ok(QueryArgs {
        pack,
        question,
        k,
        mode,
    })
}

/// Resolve the database path for `pack`, confirming it exists.
fn resolve_db_path(packs_dir: &Path, pack: &str) -> Result<PathBuf, String> {
    if pack.is_empty() || pack.contains('/') || pack.contains('\\') || pack == ".." {
        return Err(format!("invalid pack name: {pack}"));
    }
    let db_path = packs_dir.join(pack).join(DB_FILENAME);
    if !db_path.exists() {
        return Err(format!("database not found at {}", db_path.display()));
    }
    Ok(db_path)
}

fn options_for(args: &QueryArgs) -> RetrieveOptions {
    RetrieveOptions {
        k: Some(args.k),
        mode: args.mode,
        weights: None,
    }
}

/// `query <pack> <question>` — ranked retrieval over a pack's graph, as JSON.
fn cmd_query(packs_dir: &Path, args: &[String]) -> Result<String, String> {
    let parsed = parse_query_args(args)?;
    let db_path = resolve_db_path(packs_dir, &parsed.pack)?;

    let db = Database::open(&db_path).map_err(|e| e.to_string())?;
    let conn = db.connect().map_err(|e| e.to_string())?;
    let retriever = PackRetriever::new(&conn);
    let results = retriever
        .retrieve(&parsed.question, &options_for(&parsed))
        .map_err(|e| e.to_string())?;

    let json = serde_json::json!({
        "pack": parsed.pack,
        "question": parsed.question,
        "k": parsed.k,
        "results": results
            .iter()
            .map(|r| serde_json::json!({ "id": r.id, "score": r.score, "content": r.content }))
            .collect::<Vec<_>>(),
    });
    serde_json::to_string_pretty(&json).map_err(|e| e.to_string())
}

/// `ask <pack> <question>` — graph-RAG: retrieve, then synthesize a grounded
/// answer via the agent. Prints the answer + citations + supporting hits as JSON.
fn cmd_ask(
    packs_dir: &Path,
    args: &[String],
    make_transport: Option<TransportFactory<'_>>,
) -> Result<String, String> {
    let parsed = parse_query_args(args)?;
    let db_path = resolve_db_path(packs_dir, &parsed.pack)?;

    let make_transport = make_transport.ok_or_else(|| {
        "`ask` requires a synthesis transport: build with `--features copilot` or inject one via \
         run_with_transport"
            .to_string()
    })?;

    let db = Database::open(&db_path).map_err(|e| e.to_string())?;
    let conn = db.connect().map_err(|e| e.to_string())?;
    let retriever = PackRetriever::new(&conn);

    let mut agent = CopilotAgent::with_transport(make_transport(), CopilotAgentOptions::default());
    agent.start().map_err(|e| e.to_string())?;
    let answer =
        retrieve_and_synthesize(&retriever, &agent, &parsed.question, &options_for(&parsed))
            .map_err(|e| e.to_string());
    // Always stop the agent, even if synthesis failed, so the transport is shut down.
    let stop = agent.stop().map_err(|e| e.to_string());
    let answer = answer?;
    stop?;

    let json = serde_json::json!({
        "pack": parsed.pack,
        "question": parsed.question,
        "answer": answer.answer,
        "citedIds": answer.cited_ids,
        "model": answer.model,
        "results": answer
            .results
            .iter()
            .map(|r| serde_json::json!({ "id": r.id, "score": r.score, "content": r.content }))
            .collect::<Vec<_>>(),
        "usage": {
            "promptTokens": answer.usage.prompt_tokens,
            "completionTokens": answer.usage.completion_tokens,
            "reasoningTokens": answer.usage.reasoning_tokens,
            "totalTokens": answer.usage.total_tokens,
        },
    });
    serde_json::to_string_pretty(&json).map_err(|e| e.to_string())
}

/// Wire the full pack -> ingest -> retrieve -> eval flow end to end (M1 stubs).
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
    fn help_lists_query_and_ask() {
        let out = run(&["help".to_string()]).unwrap();
        assert!(out.contains("query"));
        assert!(out.contains("ask"));
    }

    #[test]
    fn demo_wires_the_pipeline() {
        let out = run(&["demo".to_string()]).unwrap();
        assert!(out.contains("ingested 1 chunk(s)"));
        assert!(out.contains("score=1"));
    }

    #[test]
    fn extract_packs_dir_pulls_the_flag_out() {
        let args: Vec<String> = ["--packs-dir", "/tmp/p", "query", "demo", "q"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let (dir, rest) = extract_packs_dir(&args);
        assert_eq!(dir, PathBuf::from("/tmp/p"));
        assert_eq!(rest, ["query", "demo", "q"]);
    }

    #[test]
    fn parse_query_args_reads_positionals_and_options() {
        let args: Vec<String> = ["demo", "what is rust?", "-k", "5", "--mode", "hybrid"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let parsed = parse_query_args(&args).unwrap();
        assert_eq!(parsed.pack, "demo");
        assert_eq!(parsed.question, "what is rust?");
        assert_eq!(parsed.k, 5);
        assert_eq!(parsed.mode, RetrieveMode::Hybrid);
    }

    #[test]
    fn parse_query_args_rejects_a_non_positive_k() {
        let args: Vec<String> = ["demo", "q", "-k", "0"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!(parse_query_args(&args).is_err());
    }

    #[test]
    fn query_errors_when_the_pack_db_is_missing() {
        let out = cmd_query(
            Path::new("/no/such/dir"),
            &["ghost".to_string(), "q".to_string()],
        );
        assert!(out.unwrap_err().contains("database not found"));
    }

    #[test]
    fn ask_without_a_transport_reports_an_error() {
        // With no transport injected and the `copilot` feature off, `ask` must
        // fail gracefully rather than panic (a missing pack DB is also a valid
        // error for this nonexistent pack).
        let out = cmd_ask(
            Path::new("/no/such/dir"),
            &["ghost".to_string(), "q".to_string()],
            None,
        );
        assert!(out.is_err());
    }
}
