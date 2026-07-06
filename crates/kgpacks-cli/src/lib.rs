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
use kgpacks_corpus::{CorpusKind, FetchOptions, DEFAULT_MAX_BYTES};
use kgpacks_db::{Database, GraphStore};
use kgpacks_embeddings::Embedder;
use kgpacks_eval::{EvalCase, Harness};
use kgpacks_ingestion::Ingestor;
use kgpacks_packs::PackManifest;
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
        "query" => cmd_query(&packs_dir, &rest[1..]),
        "ask" => cmd_ask(&packs_dir, &rest[1..], make_transport),
        "fetch-cve-corpus" => cmd_fetch_cve_corpus(&rest[1..]),
        other => Err(format!("unknown command: {other}")),
    }
}

fn help_text() -> String {
    [
        "kgpacks <command> [--packs-dir <dir>]",
        "  query <pack> <question> [-k <n>] [--mode vector|hybrid]   ranked retrieval as JSON",
        "  ask   <pack> <question> [-k <n>] [--mode vector|hybrid] [--multidoc]   graph-RAG answer as JSON",
        "  fetch-cve-corpus [--tag <tag>] [--kind baseline|delta] [--dest <dir>]   acquire the CVE corpus",
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

/// Default download/extract directory for `fetch-cve-corpus` (mirrors the reference
/// `.scratch/cve`).
const FETCH_DEFAULT_DEST: &str = ".scratch/cve";

/// The outcome of parsing `fetch-cve-corpus` arguments.
enum FetchParse {
    /// `--help` / `-h` was requested.
    Help,
    /// Validated options ready to run.
    Run(FetchOptions),
}

fn fetch_help_text() -> String {
    [
        "usage: kgpacks fetch-cve-corpus [--tag <tag>] [--kind baseline|delta] [--dest <dir>]",
        "                                [--max-bytes <N>] [--keep-archive] [--limit <N>]",
        "",
        "  --tag          release tag (default: latest release)",
        "  --kind         baseline (full corpus, default) or delta (incremental)",
        "  --dest         download/extract directory (default: .scratch/cve)",
        "  --max-bytes    hard download cap in bytes (default: 3 GiB)",
        "  --keep-archive keep the downloaded .zip after extraction",
        "  --limit        --limit to echo in the printed build command",
        "",
        "  Requires building with `--features net`. Set GITHUB_TOKEN to raise API rate limits.",
    ]
    .join("\n")
}

/// Parse + validate `fetch-cve-corpus` arguments into [`FetchOptions`].
///
/// Kept separate from the (feature-gated) execution so the whole CLI contract —
/// flag parsing and validation — is testable in the default, offline build.
fn parse_fetch_args(args: &[String]) -> Result<FetchParse, String> {
    // Returns the value following `name`, erroring when the flag is present but has no
    // value (mirrors how `parse_query_args` errors on a missing `-k` value).
    let opt = |name: &str| -> Result<Option<&String>, String> {
        match args.iter().position(|a| a == name) {
            Some(i) => match args.get(i + 1) {
                Some(value) => Ok(Some(value)),
                None => Err(format!("missing value for {name}")),
            },
            None => Ok(None),
        }
    };

    if args.iter().any(|a| a == "--help" || a == "-h") {
        return Ok(FetchParse::Help);
    }

    let tag = opt("--tag")?.cloned();
    let kind_raw = opt("--kind")?.map(String::as_str).unwrap_or("baseline");
    let kind = CorpusKind::parse(kind_raw)
        .ok_or_else(|| format!("--kind must be \"baseline\" or \"delta\" (got \"{kind_raw}\")"))?;
    let dest = opt("--dest")?
        .map(String::as_str)
        .unwrap_or(FETCH_DEFAULT_DEST);
    let keep_archive = args.iter().any(|a| a == "--keep-archive");

    let max_bytes = match opt("--max-bytes")? {
        Some(raw) => raw
            .parse::<u64>()
            .ok()
            .filter(|&n| n > 0)
            .ok_or_else(|| format!("--max-bytes must be a positive integer (got \"{raw}\")"))?,
        None => DEFAULT_MAX_BYTES,
    };
    let limit = match opt("--limit")? {
        Some(raw) => Some(
            raw.parse::<u64>()
                .ok()
                .filter(|&n| n > 0)
                .ok_or_else(|| format!("--limit must be a positive integer (got \"{raw}\")"))?,
        ),
        None => None,
    };

    Ok(FetchParse::Run(FetchOptions {
        tag,
        kind,
        dest_dir: PathBuf::from(dest),
        limit,
        keep_archive,
        max_bytes,
    }))
}

/// `fetch-cve-corpus` — acquire the CVE corpus from the CVEProject/cvelistV5 release
/// service (resolve a release, download + double-unzip the asset, record provenance,
/// print the ready-to-run build command).
fn cmd_fetch_cve_corpus(args: &[String]) -> Result<String, String> {
    match parse_fetch_args(args)? {
        FetchParse::Help => Ok(fetch_help_text()),
        FetchParse::Run(options) => run_fetch(options),
    }
}

/// Execute a fetch with the real network effects (requires the `net` feature).
#[cfg(feature = "net")]
fn run_fetch(options: FetchOptions) -> Result<String, String> {
    use kgpacks_corpus::{
        fetch_corpus, now_iso8601, GithubReleaseResolver, HttpDownloader, UnzipExtractor,
    };

    let to_msg = |e: kgpacks_corpus::CorpusError| -> String {
        match e.url() {
            Some(url) => format!("{e} ({url})"),
            None => e.to_string(),
        }
    };

    let resolver = GithubReleaseResolver::new().map_err(to_msg)?;
    let downloader = HttpDownloader::new().map_err(to_msg)?;
    let extractor = UnzipExtractor::new();

    let outcome =
        fetch_corpus(&options, &resolver, &downloader, &extractor, &now_iso8601).map_err(to_msg)?;

    let summary = serde_json::json!({
        "tag": outcome.parsed.tag,
        "kind": outcome.parsed.kind.as_str(),
        "corpusDate": outcome.parsed.corpus_date,
        "asset": outcome.parsed.asset.name,
        "bytes": outcome.bytes,
        "srcDir": outcome.src_dir.to_string_lossy(),
    });
    let summary = serde_json::to_string_pretty(&summary).map_err(|e| e.to_string())?;
    Ok(format!(
        "{summary}\n\nNext — build a pack with matching provenance:\n\n  {}\n",
        outcome.build_command
    ))
}

/// Without the `net` feature the real GitHub resolver/downloader are not compiled, so
/// report that clearly (mirroring how `ask` requires the `copilot` feature).
#[cfg(not(feature = "net"))]
fn run_fetch(_options: FetchOptions) -> Result<String, String> {
    Err(
        "`fetch-cve-corpus` requires the network integration: rebuild with `--features net` \
         (e.g. `cargo run --bin kgpacks --features net -- fetch-cve-corpus`)"
            .to_string(),
    )
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
        assert!(out.contains("fetch-cve-corpus"));
    }

    fn strvec(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn fetch_help_lists_flags() {
        let out = cmd_fetch_cve_corpus(&strvec(&["--help"])).unwrap();
        assert!(out.contains("--tag"));
        assert!(out.contains("--kind"));
        assert!(out.contains("--dest"));
        assert!(out.contains("--features net"));
    }

    #[test]
    fn fetch_parse_defaults_to_latest_baseline() {
        match parse_fetch_args(&[]).unwrap() {
            FetchParse::Run(opts) => {
                assert!(opts.tag.is_none());
                assert_eq!(opts.kind, CorpusKind::Baseline);
                assert_eq!(opts.dest_dir, PathBuf::from(FETCH_DEFAULT_DEST));
                assert_eq!(opts.max_bytes, DEFAULT_MAX_BYTES);
                assert!(!opts.keep_archive);
                assert!(opts.limit.is_none());
            }
            FetchParse::Help => panic!("did not expect help"),
        }
    }

    #[test]
    fn fetch_parse_reads_all_flags() {
        let args = strvec(&[
            "--tag",
            "cve_2026-07-03_0000Z",
            "--kind",
            "delta",
            "--dest",
            "/tmp/cve",
            "--max-bytes",
            "1000",
            "--keep-archive",
            "--limit",
            "500",
        ]);
        match parse_fetch_args(&args).unwrap() {
            FetchParse::Run(opts) => {
                assert_eq!(opts.tag.as_deref(), Some("cve_2026-07-03_0000Z"));
                assert_eq!(opts.kind, CorpusKind::Delta);
                assert_eq!(opts.dest_dir, PathBuf::from("/tmp/cve"));
                assert_eq!(opts.max_bytes, 1000);
                assert!(opts.keep_archive);
                assert_eq!(opts.limit, Some(500));
            }
            FetchParse::Help => panic!("did not expect help"),
        }
    }

    #[test]
    fn fetch_rejects_a_bad_kind() {
        assert!(parse_fetch_args(&strvec(&["--kind", "sideways"])).is_err());
    }

    #[test]
    fn fetch_rejects_a_non_positive_max_bytes() {
        assert!(parse_fetch_args(&strvec(&["--max-bytes", "0"])).is_err());
        assert!(parse_fetch_args(&strvec(&["--max-bytes", "notanumber"])).is_err());
    }

    #[test]
    fn fetch_rejects_a_non_positive_limit() {
        assert!(parse_fetch_args(&strvec(&["--limit", "0"])).is_err());
    }

    #[test]
    fn fetch_rejects_a_non_numeric_limit() {
        assert!(parse_fetch_args(&strvec(&["--limit", "notanumber"])).is_err());
    }

    #[test]
    fn fetch_rejects_a_flag_with_no_value() {
        for flag in ["--tag", "--kind", "--dest", "--max-bytes", "--limit"] {
            assert!(
                parse_fetch_args(&strvec(&[flag])).is_err(),
                "{flag} with no value should error"
            );
        }
    }

    #[cfg(not(feature = "net"))]
    #[test]
    fn fetch_without_net_reports_the_feature_requirement() {
        // Valid args, but the real network path is not compiled in the default build.
        let err = cmd_fetch_cve_corpus(&strvec(&["--kind", "delta"])).unwrap_err();
        assert!(err.contains("--features net"));
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
