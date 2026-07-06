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
//!
//! The `build` command (WS6) materializes a CVE knowledge pack from a JSON
//! corpus via [`kgpacks_packs::build_cve_pack`], with checkpoint/resume and a
//! pipelined embed-then-load; see [`cmd_build`] and the `--resume`/`--corpus`
//! flags in [`help_text`].

use std::path::{Path, PathBuf};

use kgpacks_agent::{Agent, CopilotAgent, CopilotAgentOptions, Transport};
use kgpacks_db::{Database, GraphStore};
use kgpacks_embeddings::{Embedder, DEFAULT_DIM};
use kgpacks_eval::{EvalCase, Harness};
use kgpacks_ingestion::Ingestor;
use kgpacks_packs::{
    build_cve_pack, BuildParams, FixtureCorpus, PackManifest, PipelineOptions, DEFAULT_BATCH_SIZE,
    DEFAULT_QUEUE_CAPACITY,
};
use kgpacks_query::{
    retrieve_and_synthesize, PackRetriever, RetrieveMode, RetrieveOptions, Retriever,
};

/// Filename of the LadybugDB graph store inside a pack directory (matches
/// `kgpacks_packs::GRAPH_STORE_FILENAME`).
const DB_FILENAME: &str = "graph.lbug";

/// Default number of results retrieved when `-k` is omitted (matches the
/// reference CLI's `DEFAULT_K = 5`, distinct from the library's `DEFAULT_K`).
const CLI_DEFAULT_K: usize = 5;

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
        "build" => cmd_build(&packs_dir, &rest[1..]),
        "query" => cmd_query(&packs_dir, &rest[1..]),
        "ask" => cmd_ask(&packs_dir, &rest[1..], make_transport),
        other => Err(format!("unknown command: {other}")),
    }
}

fn help_text() -> String {
    [
        "kgpacks <command> [--packs-dir <dir>]",
        "  build <pack> --corpus <file.json> [--out <dir>] [--batch <n>] [--limit <n>]",
        "        [--year <y>] [--with-entity-relations] [--queue <n>] [--pack-version <semver>]",
        "        [--resume]                                          resumable, pipelined CVE pack build",
        "  query <pack> <question> [-k <n>] [--mode vector|hybrid]   ranked retrieval as JSON",
        "  ask   <pack> <question> [-k <n>] [--mode vector|hybrid] [--multidoc]   graph-RAG answer as JSON",
        "  demo                                                      smoke-test the pipeline",
        "  version                                                   print the version",
        "",
        "build resumes from a <pack>/graph.lbug.build-checkpoint.json sidecar when --resume is",
        "given and the build parameters are unchanged; otherwise it starts a clean build. A",
        "completed pack (with a manifest) is never overwritten — remove it to rebuild.",
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
    multidoc: bool,
}

fn parse_query_args(args: &[String]) -> Result<QueryArgs, String> {
    let mut positionals: Vec<String> = Vec::new();
    let mut k = CLI_DEFAULT_K;
    let mut mode = RetrieveMode::default();
    let mut multidoc = false;

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
            // `ask` only: ground synthesis on ALL retrieved sections instead of
            // just the top one (mirrors the reference `enableMultidoc`).
            "--multidoc" => {
                multidoc = true;
                i += 1;
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
        multidoc,
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

/// Parsed arguments for the `build` command.
struct BuildArgs {
    pack: String,
    corpus: PathBuf,
    out: Option<PathBuf>,
    batch: usize,
    limit: Option<usize>,
    year: Option<i64>,
    with_entity_relations: bool,
    resume: bool,
    queue: usize,
    pack_version: String,
}

fn parse_build_args(args: &[String]) -> Result<BuildArgs, String> {
    let mut positionals: Vec<String> = Vec::new();
    let mut corpus: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut batch = DEFAULT_BATCH_SIZE;
    let mut limit: Option<usize> = None;
    let mut year: Option<i64> = None;
    let mut with_entity_relations = false;
    let mut resume = false;
    let mut queue = DEFAULT_QUEUE_CAPACITY;
    let mut pack_version = "1.0.0".to_string();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--corpus" => {
                corpus = Some(PathBuf::from(take_value(args, i, "--corpus")?));
                i += 2;
            }
            "--out" => {
                out = Some(PathBuf::from(take_value(args, i, "--out")?));
                i += 2;
            }
            "--batch" => {
                batch = parse_positive(take_value(args, i, "--batch")?, "--batch")?;
                i += 2;
            }
            "--limit" => {
                limit = Some(parse_positive(take_value(args, i, "--limit")?, "--limit")?);
                i += 2;
            }
            "--year" => {
                let raw = take_value(args, i, "--year")?;
                let value = raw
                    .parse::<i64>()
                    .map_err(|_| format!("--year must be an integer, got {raw}"))?;
                if !(1999..=2999).contains(&value) {
                    return Err(format!("--year must be in 1999..=2999, got {value}"));
                }
                year = Some(value);
                i += 2;
            }
            "--queue" => {
                // 0 is valid (rendezvous handoff between embed and load).
                let raw = take_value(args, i, "--queue")?;
                queue = raw
                    .parse::<usize>()
                    .map_err(|_| format!("--queue must be a non-negative integer, got {raw}"))?;
                i += 2;
            }
            "--pack-version" => {
                pack_version = take_value(args, i, "--pack-version")?.to_string();
                i += 2;
            }
            "--with-entity-relations" => {
                with_entity_relations = true;
                i += 1;
            }
            "--resume" => {
                resume = true;
                i += 1;
            }
            // Reject unknown flags and stray extra positionals rather than
            // silently dropping them (a typo'd flag must never "succeed").
            other if other.starts_with('-') => {
                return Err(format!("unknown flag for build: {other}"));
            }
            other => {
                if !positionals.is_empty() {
                    return Err(format!("unexpected argument: {other}"));
                }
                positionals.push(other.to_string());
                i += 1;
            }
        }
    }

    let pack = positionals
        .into_iter()
        .next()
        .ok_or_else(|| "missing <pack> argument".to_string())?;
    let corpus = corpus.ok_or_else(|| "missing required --corpus <file.json>".to_string())?;
    Ok(BuildArgs {
        pack,
        corpus,
        out,
        batch,
        limit,
        year,
        with_entity_relations,
        resume,
        queue,
        pack_version,
    })
}

fn take_value<'a>(args: &'a [String], i: usize, flag: &str) -> Result<&'a str, String> {
    match args.get(i + 1).map(String::as_str) {
        // Reject a following flag being swallowed as this flag's value; every
        // build value is a path/int/semver, so none legitimately starts with
        // `--` (this turns `--out --corpus x` into an error, not a pack named
        // `--corpus`).
        Some(value) if !value.starts_with("--") => Ok(value),
        _ => Err(format!("missing value for {flag}")),
    }
}

fn parse_positive(raw: &str, flag: &str) -> Result<usize, String> {
    raw.parse::<usize>()
        .ok()
        .filter(|&n| n >= 1)
        .ok_or_else(|| format!("{flag} must be a positive integer, got {raw}"))
}

/// Usage text for the `build` subcommand (`kgpacks build --help`).
fn build_help_text() -> String {
    [
        "kgpacks build <pack> --corpus <file.json> [options]",
        "",
        "Resumable, pipelined CVE pack build. Loads a JSON corpus and materializes",
        "a pack with checkpoint/resume and a pipelined embed-then-load.",
        "",
        "Options:",
        "  --corpus <file.json>   CVE corpus JSON array (required)",
        "  --out <dir>            output pack directory (default: <packs-dir>/<pack>)",
        "  --batch <n>            records per batch/checkpoint (default: 64)",
        "  --limit <n>            cap on corpus records considered (a prefix)",
        "  --year <y>             load only records whose published_year == y (1999..=2999)",
        "  --with-entity-relations  materialize ENTITY_RELATION edges",
        "  --queue <n>            embedded batches buffered between embed and load (default: 2)",
        "  --pack-version <ver>   manifest version (default: 1.0.0)",
        "  --resume               resume from a matching checkpoint, else clean restart",
    ]
    .join("\n")
}

/// `build <pack> --corpus <file.json>` — resumable, pipelined CVE pack build.
///
/// Loads a CVE corpus from a JSON file (the seam the external #25 fetch fills),
/// then materializes the pack with checkpoint/resume and a pipelined
/// embed-then-load. Prints a JSON build report.
fn cmd_build(packs_dir: &Path, args: &[String]) -> Result<String, String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return Ok(build_help_text());
    }
    let parsed = parse_build_args(args)?;
    if parsed.pack.is_empty()
        || parsed.pack.contains('/')
        || parsed.pack.contains('\\')
        || parsed.pack == ".."
    {
        return Err(format!("invalid pack name: {}", parsed.pack));
    }

    let pack_dir = parsed
        .out
        .clone()
        .unwrap_or_else(|| packs_dir.join(&parsed.pack));

    // Read the corpus once: parse it, and fingerprint its bytes so the build's
    // params hash binds the corpus *content* (a mid-resume edit then triggers a
    // clean restart instead of mixing old and new records).
    let raw = std::fs::read_to_string(&parsed.corpus)
        .map_err(|e| format!("cannot read corpus at {}: {e}", parsed.corpus.display()))?;
    let corpus = FixtureCorpus::from_json_str(&raw).map_err(|e| e.to_string())?;
    let src = format!(
        "{}#sha256:{}",
        parsed.corpus.display(),
        kgpacks_packs::content_fingerprint(raw.as_bytes())
    );

    // A single deterministic embedder backs the build; its model identity is
    // recorded in the params hash so a future model swap forces a clean rebuild.
    let embedder = Embedder::new(DEFAULT_DIM);
    let params = BuildParams {
        src,
        year: parsed.year,
        limit: parsed.limit,
        batch: parsed.batch,
        model: embedder.model_name().to_string(),
        with_entity_relations: parsed.with_entity_relations,
    };
    let options = PipelineOptions {
        resume: parsed.resume,
        queue_capacity: parsed.queue,
        embedding_dim: DEFAULT_DIM,
        interrupt_after_batches: None,
    };

    let manifest = PackManifest::new(parsed.pack.clone(), parsed.pack_version.clone());
    let report = build_cve_pack(&pack_dir, &manifest, &params, &corpus, &embedder, &options)
        .map_err(|e| e.to_string())?;

    let json = serde_json::json!({
        "command": "build",
        "pack": parsed.pack,
        "path": report.path.display().to_string(),
        "packVersion": parsed.pack_version,
        "paramsHash": report.params_hash,
        "resumed": report.resumed,
        "resumedFromBatch": report.resumed_from_batch,
        "interrupted": report.interrupted,
        "batchesCommitted": report.batches_committed,
        "counts": {
            "articles": report.counts.articles,
            "entities": report.counts.entities,
            "relationships": report.counts.relationships,
        },
    });
    serde_json::to_string_pretty(&json).map_err(|e| e.to_string())
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
    let answer = retrieve_and_synthesize(
        &retriever,
        &agent,
        &parsed.question,
        &options_for(&parsed),
        parsed.multidoc,
    )
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
