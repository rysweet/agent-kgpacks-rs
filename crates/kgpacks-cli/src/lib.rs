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
use kgpacks_corpus::{CorpusKind, FetchOptions, DEFAULT_MAX_BYTES};
use kgpacks_db::{Database, GraphStore};
use kgpacks_embeddings::{Embedder, DEFAULT_DIM};
use kgpacks_eval::{EvalCase, Harness};
use kgpacks_ingestion::Ingestor;
use kgpacks_packs::{
    build_cve_pack, decode_public_key, pack_release_filename, pack_release_signature_filename,
    plan_release, signature_plan, trusted_release_public_key, validate_signature_flags,
    verify_pack_index_signature, BuildParams, FixtureCorpus, PackIndexSignature, PackManifest,
    PipelineOptions, ProvenanceOverrides, SignatureInputs, SignaturePlan, DEFAULT_BATCH_SIZE,
    DEFAULT_QUEUE_CAPACITY, LATEST_POINTER_TAG,
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

/// Environment variable overriding the trusted release public key (base64 raw
/// 32-byte Ed25519). Falls back to the committed
/// `kgpacks-packs/keys/pack-release-signing.pub` when unset.
const TRUSTED_KEY_ENV: &str = "KGPACKS_TRUSTED_RELEASE_KEY";

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
        "status" => cmd_status(&packs_dir),
        "pack" => cmd_pack(&packs_dir, &rest[1..]),
        "ask" => cmd_ask(&packs_dir, &rest[1..], make_transport),
        "fetch-cve-corpus" => cmd_fetch_cve_corpus(&rest[1..]),
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
        "  fetch-cve-corpus [--tag <tag>] [--kind baseline|delta] [--dest <dir>]   acquire the CVE corpus",
        "  status                                                    installed packs summary as JSON",
        "  pack list                                                 installed packs (name, version, description) as JSON",
        "  pack info <pack>                                          a pack's full manifest as JSON",
        "  pack validate <pack>                                      validate a pack's manifest as JSON",
        "  pack release-plan <pack> [--tag <t>]                      offline pack-release plan (version, provenance, tags) as JSON",
        "  pack pull <pack> [--require-signature] [--no-verify] [--trusted-key <b64>]   verify + pull a signed release index",
        "  demo                                                      smoke-test the pipeline",
        "  version                                                   print the version",
        "",
        "build resumes from a <pack>/graph.lbug.build-checkpoint.json sidecar when --resume is",
        "given and the build parameters are unchanged; otherwise it starts a clean build. A",
        "completed pack (with a manifest) is never overwritten — remove it to rebuild.",
        "",
        "Packs directory resolution (highest precedence first):",
        "  --packs-dir <dir>        explicit override (this flag)",
        "  KGPACKS_PACKS_DIR=<dir>  environment override",
        "  default                  $XDG_DATA_HOME/kgpacks, else ~/.local/share/kgpacks",
        "Blank (empty/whitespace-only) overrides are ignored.",
    ]
    .join("\n")
}

/// Pull a global `--packs-dir <dir>` flag out of `args` (anywhere in the list),
/// returning the resolved packs directory and the remaining arguments.
///
/// The flag is the explicit override; when it is absent (or blank), resolution
/// falls through to the `KGPACKS_PACKS_DIR` env var and then the XDG default
/// (`$XDG_DATA_HOME/kgpacks`, else `~/.local/share/kgpacks`). This shared
/// resolution ([`kgpacks_packs::resolve_packs_dir`]) is the SAME one the MCP
/// server uses, so a pack installed by one is found by the other.
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
    let dir = kgpacks_packs::resolve_packs_dir(flag.as_deref());
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

/// Validate a pack name as a single safe path segment (no traversal).
fn validate_pack_name(pack: &str) -> Result<(), String> {
    if pack.is_empty() || pack.contains('/') || pack.contains('\\') || pack == ".." {
        return Err(format!("invalid pack name: {pack}"));
    }
    Ok(())
}

/// Resolve the database path for `pack`, confirming it exists.
///
/// Ensures the packs directory itself exists first (best-effort) so the default
/// XDG location is created on first use; a missing pack still surfaces a clear
/// "database not found" error.
fn resolve_db_path(packs_dir: &Path, pack: &str) -> Result<PathBuf, String> {
    validate_pack_name(pack)?;
    kgpacks_packs::ensure_packs_dir(packs_dir);
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

/// `status` — resolved packs directory plus a per-pack summary.
///
/// Ports `cli/src/commands/status.ts`: lists the installed packs under the
/// resolved packs directory (a missing directory yields an empty list, never an
/// error) and reports, for each, whether its LadybugDB graph store is present.
/// Output is pretty JSON: `{ packsDir, count, packs: [{ name, version,
/// dbPresent }] }`, with `packs` sorted by name via
/// [`localecompare_pack_name`] (a faithful port of the reference's
/// `name.localeCompare`).
fn cmd_status(packs_dir: &Path) -> Result<String, String> {
    let mut packs = kgpacks_packs::list_packs(packs_dir);
    packs.sort_by(|a, b| localecompare_pack_name(&a.name, &b.name));

    let json = serde_json::json!({
        "packsDir": packs_dir.display().to_string(),
        "count": packs.len(),
        "packs": packs
            .iter()
            .map(|p| {
                serde_json::json!({
                    "name": p.name,
                    "version": p.version,
                    "dbPresent": p.path.join(DB_FILENAME).exists(),
                })
            })
            .collect::<Vec<_>>(),
    });
    serde_json::to_string_pretty(&json).map_err(|e| e.to_string())
}

/// `pack <subcommand>` — read-path registry management + offline release plan.
///
/// Ports the READ-PATH subset of `cli/src/commands/pack.ts` plus the offline
/// projection of the release tooling (`scripts/release-pack.mjs`): the
/// deterministic, network-free subcommands `list`, `info`, `validate`, and
/// `release-plan`, plus WS7 (#22) `pull`, which verifies the release-index
/// Ed25519 signature (fail-closed) before trusting it. The byte-level packaging
/// and `gh` upload behind `release-plan`, the remaining write/network
/// subcommands (`install`, `remove`) and the ingestion/eval verbs (`create`,
/// `update`, `eval`) remain follow-ups (issue #13), consistent with the Rust
/// port being the read-path subset of the TypeScript CLI.
fn cmd_pack(packs_dir: &Path, args: &[String]) -> Result<String, String> {
    match args.first().map(String::as_str) {
        Some("list") => cmd_pack_list(packs_dir),
        Some("info") => cmd_pack_info(packs_dir, args.get(1)),
        Some("validate") => cmd_pack_validate(packs_dir, args.get(1)),
        Some("release-plan") => cmd_pack_release_plan(packs_dir, &args[1..]),
        Some("pull") => cmd_pack_pull(packs_dir, &args[1..]),
        Some(other) => Err(format!(
            "unknown pack subcommand: {other} (expected: list, info, validate, release-plan, pull)"
        )),
        None => Err(
            "missing pack subcommand (expected: list, info, validate, release-plan, pull)"
                .to_string(),
        ),
    }
}

/// `pack list` — the installed packs as `[{ name, version, description }]`.
///
/// Ports `pack list` from `cli/src/commands/pack.ts`: lists every pack under the
/// resolved packs directory (a missing directory yields an empty list, never an
/// error), projects each to `{ name, version, description }` (description
/// defaulting to `""` when absent, mirroring the reference's
/// `typeof … === 'string' ? … : ''`), and sorts by name via
/// [`localecompare_pack_name`] (the same ICU-root order as `status`). Output is
/// pretty JSON.
fn cmd_pack_list(packs_dir: &Path) -> Result<String, String> {
    let mut packs = kgpacks_packs::list_packs(packs_dir);
    packs.sort_by(|a, b| localecompare_pack_name(&a.name, &b.name));

    let json = serde_json::Value::Array(
        packs
            .iter()
            .map(|p| {
                serde_json::json!({
                    "name": p.name,
                    "version": p.version,
                    "description": p.manifest.description.clone().unwrap_or_default(),
                })
            })
            .collect(),
    );
    serde_json::to_string_pretty(&json).map_err(|e| e.to_string())
}

/// `pack info <pack>` — a pack's full manifest as pretty JSON.
///
/// Ports `pack info` from `cli/src/commands/pack.ts` (over `packs/registry.ts`
/// `packInfo`): validates the pack name against `PACK_NAME_RE`, requires the
/// pack's `manifest.json` to exist (else `pack not found`), then loads +
/// validates the manifest and prints it in the canonical snake_case on-disk
/// shape (`PackManifest::to_value`), at parity with the reference's
/// `printJson(info.manifest)`.
fn cmd_pack_info(packs_dir: &Path, name: Option<&String>) -> Result<String, String> {
    let name = name.ok_or_else(|| "missing <pack> argument".to_string())?;
    if !kgpacks_packs::pack_name_re().is_match(name) {
        return Err(format!("invalid pack name: {name}"));
    }
    let dir = packs_dir.join(name);
    if !kgpacks_packs::manifest_path_in(&dir).exists() {
        return Err(format!("pack not found: {name}"));
    }
    let manifest = kgpacks_packs::load_manifest_from_dir(&dir).map_err(|e| e.to_string())?;
    serde_json::to_string_pretty(&manifest.to_value()).map_err(|e| e.to_string())
}

/// `pack validate <pack>` — confirm a pack's manifest is valid.
///
/// Ports `pack validate` from `cli/src/commands/pack.ts`: resolves an existing
/// pack directory (a name that fails `PACK_NAME_RE`, or a missing directory /
/// `manifest.json`, is reported as `pack not found`), loads + validates the
/// manifest (a schema violation surfaces as an error), and prints
/// `{ valid: true, name, version }` on success.
fn cmd_pack_validate(packs_dir: &Path, name: Option<&String>) -> Result<String, String> {
    let name = name.ok_or_else(|| "missing <pack> argument".to_string())?;
    if !kgpacks_packs::pack_name_re().is_match(name) {
        return Err(format!("pack not found: {name}"));
    }
    let dir = packs_dir.join(name);
    if !dir.is_dir() || !kgpacks_packs::manifest_path_in(&dir).exists() {
        return Err(format!("pack not found: {name}"));
    }
    let manifest = kgpacks_packs::load_manifest_from_dir(&dir).map_err(|e| e.to_string())?;

    let json = serde_json::json!({
        "valid": true,
        "name": manifest.name,
        "version": manifest.version,
    });
    serde_json::to_string_pretty(&json).map_err(|e| e.to_string())
}

/// `pack release-plan <pack> [--tag <tag>] [--model <id>] [--corpus-commit <sha>]
/// [--corpus-date <date>]` — the offline plan for publishing a pack release.
///
/// Surfaces [`kgpacks_packs::plan_release`]: the pure, network-free projection
/// of `scripts/release-pack.mjs` (the half a `--dry-run` computes before
/// packaging). It resolves the published `version` from the (dated) `--tag`
/// (defaulting to the stable `packs` latest-pointer), mirrors the manifest build
/// `provenance` into the release index (filling gaps from the overrides + a
/// release-time `build.date`), and reports the `publishTargets` (the dated tag
/// plus the `packs` pointer so `pack pull` still resolves latest) and the
/// `<name>.pack-release.json` `indexFilename`. Output is pretty JSON; the
/// byte-level packaging + `gh` upload remain follow-ups (issue #13).
fn cmd_pack_release_plan(packs_dir: &Path, args: &[String]) -> Result<String, String> {
    let mut name: Option<&String> = None;
    let mut tag = LATEST_POINTER_TAG.to_string();
    let mut overrides = ProvenanceOverrides::default();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--tag" => tag = flag_value(args, &mut i, "--tag")?,
            "--model" => overrides.model = Some(flag_value(args, &mut i, "--model")?),
            "--corpus-commit" => {
                overrides.corpus_commit = Some(flag_value(args, &mut i, "--corpus-commit")?)
            }
            "--corpus-date" => {
                overrides.corpus_date = Some(flag_value(args, &mut i, "--corpus-date")?)
            }
            other if other.starts_with("--") => return Err(format!("unknown flag: {other}")),
            _ => {
                if name.is_some() {
                    return Err(format!("unexpected extra argument: {}", args[i]));
                }
                name = Some(&args[i]);
            }
        }
        i += 1;
    }

    let name = name.ok_or_else(|| "missing <pack> argument".to_string())?;
    if !kgpacks_packs::pack_name_re().is_match(name) {
        return Err(format!("invalid pack name: {name}"));
    }
    let dir = packs_dir.join(name);
    if !kgpacks_packs::manifest_path_in(&dir).exists() {
        return Err(format!("pack not found: {name}"));
    }

    let now = kgpacks_packs::now_iso8601_utc();
    let plan = plan_release(&dir, &tag, &overrides, &now).map_err(|e| e.to_string())?;
    serde_json::to_string_pretty(&plan.to_value()).map_err(|e| e.to_string())
}

/// Read the value following a `--flag` at `args[*i]`, advancing `*i` past it.
fn flag_value(args: &[String], i: &mut usize, flag: &str) -> Result<String, String> {
    *i += 1;
    args.get(*i)
        .cloned()
        .ok_or_else(|| format!("{flag} requires a value"))
}

/// Compare two pack names the way the reference CLI's `name.localeCompare(...)`
/// does, restricted to the ASCII pack-name character set (`PACK_NAME_RE`:
/// `[a-zA-Z0-9_-]`).
///
/// JavaScript's `String.prototype.localeCompare` uses ICU collation. For this
/// character set the ICU **root / `en-US`** order — which is deterministic and
/// host-independent, unlike the reference's implicit host-default locale — has
/// two effective levels:
///
/// * a case-insensitive **primary** order of `_` < `-` < digits < letters, and
/// * a **case** tiebreak with lowercase before uppercase.
///
/// Because pack names are pure ASCII, no locale-specific casing (e.g. Turkish
/// `i`/`İ`) can arise, so pinning to the root/`en-US` order is a faithful,
/// deterministic port. Crucially the *entire* primary sequence is compared
/// before any case difference, so e.g. `"Ab" < "ac"` (primary `b` < `c` wins
/// over the uppercase `A`). A naive per-character `(primary, case)` tuple sort
/// gets that wrong; this reproduces the level-by-level order exactly. It is
/// verified against Node's real `localeCompare('en-US')` by the
/// `qa/status-parity` harness (and a 200k-pair differential check).
fn localecompare_pack_name(a: &str, b: &str) -> std::cmp::Ordering {
    // Primary collation weight: case-folded, `_` < `-` < digits < letters. Any
    // character outside a valid pack name (never produced here, since names pass
    // `PACK_NAME_RE`) sorts after all known ones, deterministically by code
    // point, so the comparator stays total.
    fn primary(c: char) -> u32 {
        match c {
            '_' => 0,
            '-' => 1,
            '0'..='9' => 2 + (c as u32 - '0' as u32),
            'a'..='z' => 12 + (c as u32 - 'a' as u32),
            'A'..='Z' => 12 + (c as u32 - 'A' as u32),
            other => 1000 + other as u32,
        }
    }
    // Case (tertiary) weight: lowercase and non-letters before uppercase.
    fn case_rank(c: char) -> u8 {
        u8::from(c.is_ascii_uppercase())
    }

    a.chars()
        .map(primary)
        .cmp(b.chars().map(primary))
        .then_with(|| a.chars().map(case_rank).cmp(b.chars().map(case_rank)))
}
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

/// Flags parsed for `pack pull`.
struct PullArgs {
    pack: String,
    require_signature: bool,
    no_verify: bool,
    trusted_key: Option<String>,
}

fn parse_pull_args(args: &[String]) -> Result<PullArgs, String> {
    let mut positionals: Vec<String> = Vec::new();
    let mut require_signature = false;
    let mut no_verify = false;
    let mut trusted_key: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--require-signature" => {
                require_signature = true;
                i += 1;
            }
            "--no-verify" => {
                no_verify = true;
                i += 1;
            }
            "--trusted-key" => {
                let raw = args
                    .get(i + 1)
                    .ok_or_else(|| "missing value for --trusted-key".to_string())?;
                trusted_key = Some(raw.clone());
                i += 2;
            }
            other => {
                positionals.push(other.to_string());
                i += 1;
            }
        }
    }

    let pack = positionals
        .into_iter()
        .next()
        .ok_or_else(|| "missing <pack> argument".to_string())?;
    Ok(PullArgs {
        pack,
        require_signature,
        no_verify,
        trusted_key,
    })
}

/// Resolve the trusted release public key: `--trusted-key` flag wins, then the
/// `KGPACKS_TRUSTED_RELEASE_KEY` env var, else the committed trusted key.
fn resolve_trusted_key(flag: &Option<String>) -> Result<[u8; 32], String> {
    if let Some(raw) = flag {
        return decode_public_key(raw).map_err(|e| e.to_string());
    }
    if let Ok(raw) = std::env::var(TRUSTED_KEY_ENV) {
        return decode_public_key(&raw).map_err(|e| e.to_string());
    }
    Ok(trusted_release_public_key())
}

/// `pack pull <pack>` — verify the release index signature, then "pull".
///
/// Applies the fail-closed signature policy over the RAW release-index bytes
/// (verify-before-parse), hard-failing on a tampered/untrusted signature or on a
/// missing signature under `--require-signature`. On Verify/Warn/Skip it then
/// does a minimal, format-agnostic parse of the index (well-formed JSON), so the
/// verification composes additively with the WS3 release-index schema.
fn cmd_pack_pull(packs_dir: &Path, args: &[String]) -> Result<String, String> {
    let parsed = parse_pull_args(args)?;
    // Mutually-exclusive flags are a usage error, checked before any I/O.
    validate_signature_flags(parsed.require_signature, parsed.no_verify)
        .map_err(|e| e.to_string())?;
    validate_pack_name(&parsed.pack)?;

    let trusted_key = resolve_trusted_key(&parsed.trusted_key)?;

    let index_path = packs_dir.join(pack_release_filename(&parsed.pack));
    if !index_path.exists() {
        return Err(format!(
            "release index not found at {}",
            index_path.display()
        ));
    }
    let index_bytes = std::fs::read(&index_path)
        .map_err(|e| format!("cannot read release index at {}: {e}", index_path.display()))?;

    let sig_path = packs_dir.join(pack_release_signature_filename(&parsed.pack));
    let present = sig_path.exists();

    // Compute validity by verifying the RAW bytes before any parse. A malformed
    // or wrong-length sidecar counts as an invalid (not absent) signature so the
    // policy fails closed rather than silently downgrading to integrity-only.
    // Under `--no-verify` we never touch the sidecar — skipping means skipping,
    // so an unreadable sidecar must not turn a `--no-verify` pull into an error.
    let valid = if present && !parsed.no_verify {
        let sig_raw = std::fs::read_to_string(&sig_path)
            .map_err(|e| format!("cannot read signature sidecar: {e}"))?;
        match PackIndexSignature::from_json_str(&sig_raw)
            .and_then(|sidecar| sidecar.signature_array())
        {
            Ok(sig) => verify_pack_index_signature(&index_bytes, &sig, &trusted_key),
            Err(_) => false,
        }
    } else {
        false
    };

    let plan = signature_plan(SignatureInputs {
        present,
        valid,
        require_signature: parsed.require_signature,
        no_verify: parsed.no_verify,
    });

    let policy = match plan {
        SignaturePlan::Verify => "verify",
        SignaturePlan::Fail => "fail",
        SignaturePlan::Warn => "warn",
        SignaturePlan::Skip => "skip",
    };

    if plan == SignaturePlan::Fail {
        return Err(if present {
            format!(
                "signature verification failed for \"{}\" release index (fail-closed)",
                parsed.pack
            )
        } else {
            format!(
                "\"{}\" release index is unsigned but --require-signature was set",
                parsed.pack
            )
        });
    }

    // Verify-before-parse: only now, on a Verify/Warn/Skip outcome, interpret
    // the index bytes. This stays format-agnostic — any well-formed JSON index
    // is accepted, regardless of the WS3 schema details.
    let index: serde_json::Value = serde_json::from_slice(&index_bytes)
        .map_err(|e| format!("release index is not valid JSON: {e}"))?;

    let json = serde_json::json!({
        "command": "pull",
        "pack": parsed.pack,
        "signature": {
            "present": present,
            "verified": plan == SignaturePlan::Verify,
            "policy": policy,
        },
        "index": {
            "name": index.get("name").and_then(serde_json::Value::as_str),
            "format": index.get("format").and_then(serde_json::Value::as_str),
        },
        "status": "ok",
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
        assert!(out.contains("fetch-cve-corpus"));
        assert!(out.contains("status"));
        assert!(out.contains("pack list"));
        assert!(out.contains("pack info"));
        assert!(out.contains("pack validate"));
        assert!(out.contains("pack release-plan"));
        assert!(out.contains("pack pull"));
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
    fn status_on_a_missing_packs_dir_is_empty() {
        // A nonexistent packs directory is not an error: `status` reports zero
        // packs (mirrors the reference, whose `listPacks` returns `[]`).
        let out = cmd_status(Path::new("/no/such/packs/dir")).unwrap();
        let value: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(value["packsDir"], "/no/such/packs/dir");
        assert_eq!(value["count"], 0);
        assert_eq!(value["packs"], serde_json::json!([]));
    }

    #[test]
    fn status_lists_installed_packs_sorted_with_db_presence() {
        use std::fs;
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();

        // Two valid packs; only `zeta` has a graph store present.
        let zeta = root.join("zeta");
        fs::create_dir_all(&zeta).unwrap();
        fs::write(
            zeta.join("manifest.json"),
            r#"{"name":"zeta","version":"2.0.0"}"#,
        )
        .unwrap();
        fs::write(zeta.join(DB_FILENAME), b"").unwrap();

        let alpha = root.join("alpha");
        fs::create_dir_all(&alpha).unwrap();
        fs::write(
            alpha.join("manifest.json"),
            r#"{"name":"alpha","version":"1.0.0"}"#,
        )
        .unwrap();

        // A directory without a manifest is skipped.
        fs::create_dir_all(root.join("not-a-pack")).unwrap();

        let out = cmd_status(root).unwrap();
        let value: serde_json::Value = serde_json::from_str(&out).unwrap();

        assert_eq!(value["packsDir"], root.display().to_string());
        assert_eq!(value["count"], 2);
        let packs = value["packs"].as_array().unwrap();
        assert_eq!(packs.len(), 2);
        // Sorted by name: alpha before zeta.
        assert_eq!(packs[0]["name"], "alpha");
        assert_eq!(packs[0]["version"], "1.0.0");
        assert_eq!(packs[0]["dbPresent"], false);
        assert_eq!(packs[1]["name"], "zeta");
        assert_eq!(packs[1]["version"], "2.0.0");
        assert_eq!(packs[1]["dbPresent"], true);
    }

    #[test]
    fn pack_list_projects_name_version_description_sorted() {
        use std::fs;
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();

        let a = root.join("alpha");
        fs::create_dir_all(&a).unwrap();
        fs::write(
            a.join("manifest.json"),
            r#"{"name":"alpha","version":"1.0.0","description":"the alpha pack"}"#,
        )
        .unwrap();

        let z = root.join("zeta");
        fs::create_dir_all(&z).unwrap();
        // No description -> defaults to "".
        fs::write(
            z.join("manifest.json"),
            r#"{"name":"zeta","version":"2.0.0"}"#,
        )
        .unwrap();

        // Invalid manifest (missing version) -> skipped, never fatal.
        let b = root.join("broken");
        fs::create_dir_all(&b).unwrap();
        fs::write(b.join("manifest.json"), r#"{"name":"broken"}"#).unwrap();

        let out = cmd_pack_list(root).unwrap();
        let value: serde_json::Value = serde_json::from_str(&out).unwrap();
        let packs = value.as_array().unwrap();
        assert_eq!(packs.len(), 2);
        assert_eq!(packs[0]["name"], "alpha");
        assert_eq!(packs[0]["version"], "1.0.0");
        assert_eq!(packs[0]["description"], "the alpha pack");
        assert_eq!(packs[1]["name"], "zeta");
        // Absent description is the empty string, not null / missing.
        assert_eq!(packs[1]["description"], "");
    }

    #[test]
    fn pack_list_on_a_missing_packs_dir_is_empty() {
        let out = cmd_pack_list(Path::new("/no/such/packs/dir")).unwrap();
        assert_eq!(out, "[]");
    }

    #[test]
    fn pack_info_prints_the_full_manifest_verbatim() {
        use std::fs;
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        let dir = root.join("rich");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("manifest.json"),
            r#"{"name":"rich","version":"1.2.3","description":"rich","graph_stats":{"articles":7,"size_mb":1.5},"unknown":"kept"}"#,
        )
        .unwrap();

        let name = "rich".to_string();
        let out = cmd_pack_info(root, Some(&name)).unwrap();
        let value: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(value["name"], "rich");
        assert_eq!(value["version"], "1.2.3");
        assert_eq!(value["graph_stats"]["articles"], 7);
        assert_eq!(value["graph_stats"]["size_mb"], 1.5);
        // Unknown keys survive the round-trip.
        assert_eq!(value["unknown"], "kept");
    }

    #[test]
    fn pack_info_missing_argument_and_missing_pack_error() {
        let tmp = tempfile::tempdir().expect("tempdir");
        assert_eq!(
            cmd_pack_info(tmp.path(), None).unwrap_err(),
            "missing <pack> argument"
        );
        let name = "ghost".to_string();
        assert_eq!(
            cmd_pack_info(tmp.path(), Some(&name)).unwrap_err(),
            "pack not found: ghost"
        );
    }

    #[test]
    fn pack_info_rejects_an_invalid_name_before_touching_the_filesystem() {
        let name = "../escape".to_string();
        assert_eq!(
            cmd_pack_info(Path::new("/no/such/root"), Some(&name)).unwrap_err(),
            "invalid pack name: ../escape"
        );
    }

    #[test]
    fn pack_validate_accepts_valid_and_rejects_invalid() {
        use std::fs;
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();

        let good = root.join("good");
        fs::create_dir_all(&good).unwrap();
        fs::write(
            good.join("manifest.json"),
            r#"{"name":"good","version":"3.1.4"}"#,
        )
        .unwrap();

        let name = "good".to_string();
        let out = cmd_pack_validate(root, Some(&name)).unwrap();
        let value: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(value["valid"], true);
        assert_eq!(value["name"], "good");
        assert_eq!(value["version"], "3.1.4");

        // Missing required `version` -> validation error surfaces.
        let bad = root.join("bad");
        fs::create_dir_all(&bad).unwrap();
        fs::write(bad.join("manifest.json"), r#"{"name":"bad"}"#).unwrap();
        let bad_name = "bad".to_string();
        assert!(cmd_pack_validate(root, Some(&bad_name)).is_err());
    }

    #[test]
    fn pack_validate_reports_a_traversal_name_as_not_found() {
        // Both an invalid name and a missing directory are reported identically
        // ("pack not found"), matching the reference's resolveExistingPackDir.
        let traversal = "../secrets".to_string();
        assert_eq!(
            cmd_pack_validate(Path::new("/no/such/root"), Some(&traversal)).unwrap_err(),
            "pack not found: ../secrets"
        );
        let missing = "ghost".to_string();
        let tmp = tempfile::tempdir().expect("tempdir");
        assert_eq!(
            cmd_pack_validate(tmp.path(), Some(&missing)).unwrap_err(),
            "pack not found: ghost"
        );
    }

    #[test]
    fn pack_dispatch_rejects_bare_and_unknown_subcommands() {
        assert!(cmd_pack(Path::new("/tmp"), &[])
            .unwrap_err()
            .contains("missing pack subcommand"));
        assert!(cmd_pack(Path::new("/tmp"), &["install".to_string()])
            .unwrap_err()
            .contains("unknown pack subcommand: install"));
    }

    #[test]
    fn pack_release_plan_projects_dated_tag_and_mirrors_provenance() {
        use std::fs;
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        let pack = root.join("cve");
        fs::create_dir_all(&pack).unwrap();
        fs::write(
            pack.join("manifest.json"),
            r#"{"name":"cve","version":"0.1.0","provenance":{"corpus":{"name":"cvelistV5","commit":"abc123","date":"2026-01-01"},"embedding":{"model":"Xenova/bge-base-en-v1.5","dimensions":768},"build":{"date":"2026-01-02T00:00:00Z","tool_version":"0.1.0"}}}"#,
        )
        .unwrap();

        let args = ["release-plan", "cve", "--tag", "cve-2025.06"].map(String::from);
        let out = cmd_pack(root, &args).unwrap();
        let value: serde_json::Value = serde_json::from_str(&out).unwrap();

        assert_eq!(value["name"], "cve");
        assert_eq!(value["tag"], "cve-2025.06");
        // Dated tag → UNPADDED SemVer core.
        assert_eq!(value["version"], "2025.6.0");
        // Dated release moves the stable `packs` latest-pointer too.
        assert_eq!(
            value["publishTargets"],
            serde_json::json!(["cve-2025.06", "packs"])
        );
        assert_eq!(value["indexFilename"], "cve.pack-release.json");
        // Provenance mirrored from the manifest (build.date present → not defaulted).
        assert_eq!(value["provenance"]["corpus"]["commit"], "abc123");
        assert_eq!(value["provenance"]["embedding"]["dimensions"], 768);
        assert_eq!(value["provenance"]["build"]["date"], "2026-01-02T00:00:00Z");
    }

    #[test]
    fn pack_release_plan_defaults_to_the_packs_pointer() {
        use std::fs;
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        let pack = root.join("cve");
        fs::create_dir_all(&pack).unwrap();
        fs::write(
            pack.join("manifest.json"),
            r#"{"name":"cve","version":"7.7.7","provenance":{"build":{"date":"2026-01-02T00:00:00Z"}}}"#,
        )
        .unwrap();

        // No --tag: defaults to the `packs` latest-pointer, which carries no
        // dated version so the plan falls back to the manifest version.
        let args = ["release-plan", "cve"].map(String::from);
        let out = cmd_pack(root, &args).unwrap();
        let value: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(value["tag"], "packs");
        assert_eq!(value["version"], "7.7.7");
        assert_eq!(value["publishTargets"], serde_json::json!(["packs"]));
    }

    #[test]
    fn pack_release_plan_reports_missing_and_invalid_packs() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // Missing pack.
        let args = ["release-plan", "ghost"].map(String::from);
        assert_eq!(
            cmd_pack(tmp.path(), &args).unwrap_err(),
            "pack not found: ghost"
        );
        // Missing <pack> argument.
        let args = ["release-plan"].map(String::from);
        assert!(cmd_pack(tmp.path(), &args)
            .unwrap_err()
            .contains("missing <pack> argument"));
        // Unknown flag.
        let args = ["release-plan", "cve", "--nope"].map(String::from);
        assert!(cmd_pack(tmp.path(), &args)
            .unwrap_err()
            .contains("unknown flag: --nope"));
    }

    #[test]
    fn localecompare_pack_name_matches_icu_primary_order() {
        use std::cmp::Ordering;
        // Primary order: `_` < `-` < digits < letters (case-insensitive).
        assert_eq!(localecompare_pack_name("_x", "-x"), Ordering::Less); // `_` < `-`
        assert_eq!(localecompare_pack_name("a_1", "a1"), Ordering::Less); // `_` < digit
        assert_eq!(
            localecompare_pack_name("my_pack", "my-pack"),
            Ordering::Less
        );
        assert_eq!(localecompare_pack_name("9x", "ax"), Ordering::Less); // digit < letter
        assert_eq!(localecompare_pack_name("apple", "banana"), Ordering::Less);
    }

    #[test]
    fn localecompare_pack_name_case_tiebreak_is_lowercase_first() {
        use std::cmp::Ordering;
        // Equal ignoring case -> lowercase sorts first (ICU tertiary level).
        assert_eq!(localecompare_pack_name("alpha", "Alpha"), Ordering::Less);
        assert_eq!(localecompare_pack_name("Alpha", "alpha"), Ordering::Greater);
        // But a primary difference dominates a case difference anywhere.
        assert_eq!(localecompare_pack_name("Ab", "ac"), Ordering::Less);
        assert_eq!(localecompare_pack_name("aB", "ab"), Ordering::Greater);
    }

    #[test]
    fn localecompare_pack_name_orders_a_mixed_set_like_the_reference() {
        // Same set + expected order as computed from Node's `localeCompare`
        // (see qa/status-parity).
        let mut names = ["alpha", "Alpha", "a1", "a_1", "my-pack", "my_pack", "bravo"];
        names.sort_by(|a, b| localecompare_pack_name(a, b));
        assert_eq!(
            names,
            ["a_1", "a1", "alpha", "Alpha", "bravo", "my_pack", "my-pack"]
        );
    }

    #[test]
    fn status_sorts_names_by_localecompare_not_codepoint() {
        use std::fs;
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        for name in ["alpha", "Alpha", "my-pack", "my_pack"] {
            let dir = root.join(name);
            fs::create_dir_all(&dir).unwrap();
            fs::write(
                dir.join("manifest.json"),
                format!(r#"{{"name":"{name}","version":"1.0.0"}}"#),
            )
            .unwrap();
        }
        let out = cmd_status(root).unwrap();
        let value: serde_json::Value = serde_json::from_str(&out).unwrap();
        let order: Vec<&str> = value["packs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["name"].as_str().unwrap())
            .collect();
        // localeCompare order: lowercase before uppercase; `_` before `-`.
        assert_eq!(order, ["alpha", "Alpha", "my_pack", "my-pack"]);
    }

    #[test]
    fn parse_pull_args_reads_flags_and_positional() {
        let parsed = parse_pull_args(&args_vec(&[
            "acme",
            "--require-signature",
            "--trusted-key",
            "AAAA",
        ]))
        .unwrap();
        assert_eq!(parsed.pack, "acme");
        assert!(parsed.require_signature);
        assert!(!parsed.no_verify);
        assert_eq!(parsed.trusted_key.as_deref(), Some("AAAA"));
    }

    #[test]
    fn parse_pull_args_requires_a_pack() {
        assert!(parse_pull_args(&args_vec(&["--no-verify"])).is_err());
    }

    fn args_vec(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
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
