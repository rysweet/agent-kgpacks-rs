# agent-kgpacks-rs

A Rust port of [`agent-kgpacks`](https://github.com/rysweet/agent-kgpacks-ts) — a
knowledge-pack platform that builds a **LadybugDB** graph from source documents and
answers questions with a **graph-RAG agent** over the GitHub Copilot SDK.

The executable specification for this port is the TypeScript reference
[`rysweet/agent-kgpacks-ts`](https://github.com/rysweet/agent-kgpacks-ts) (a pnpm
workspace). This repository mirrors that module decomposition as a **Cargo
workspace** and reuses the Simard Rust stack (`lbug` for the graph + vector/FTS
engine, RustyClawd / Copilot SDK for the agent).

> **Status:** M5 — graph-RAG agent + CLI parity. On top of the M2 graph store,
> M3 ingestion, and M4 retrieval, `kgpacks-agent` ports the Copilot-SDK agent
> (answer synthesis, query expansion, multi-query, seed-article identification,
> usage accounting, fail-closed error model) behind an injectable transport
> seam, wired to the real RustyClawd / Copilot backend behind the `copilot`
> feature. `kgpacks-query` adds the agent-grounded **graph-RAG query**
> (`retrieve_and_synthesize`), and `kgpacks-cli` surfaces it end to end: `query`
> prints ranked retrieval, `ask` prints a grounded, citation-bearing answer, and
> `status` reports the installed packs.
> See the [roadmap](#porting-roadmap-m1m5) for what lands when.

## Target flow

The platform pipeline this port targets, end to end:

```
build pack ──▶ ingest ──▶ graph + vector + FTS retrieval ──▶ graph-RAG query
   packs      ingestion           db + query                  query + agent
```

1. **build pack** — assemble a knowledge pack (manifest + sources) — `kgpacks-packs`.
2. **ingest** — fetch, chunk, extract, embed and load documents into the store —
   `kgpacks-ingestion` + `kgpacks-embeddings`.
3. **graph + vector + FTS retrieval** — store the knowledge graph and run hybrid
   retrieval (vector similarity + full-text + Cypher) with reranking —
   `kgpacks-db` + `kgpacks-query`.
4. **graph-RAG query** — synthesize a grounded answer with the agent —
   `kgpacks-query` + `kgpacks-agent`, exposed via `kgpacks-cli`, `kgpacks-mcp` and
   `kgpacks-backend`.

## Workspace layout

Each Rust crate corresponds to a TypeScript package in the reference. Crates live
under [`crates/`](crates/) and share versioning through the root
[`Cargo.toml`](Cargo.toml) `[workspace]`.

| Rust crate           | TS package           | Responsibility                                                |
| -------------------- | -------------------- | ------------------------------------------------------------- |
| `kgpacks-db`         | `@kgpacks/db`        | Graph + vector + FTS store (LadybugDB / `lbug`).              |
| `kgpacks-embeddings` | `@kgpacks/embeddings`| Text embeddings (HuggingFace-transformers parity).            |
| `kgpacks-packs`      | `@kgpacks/packs`     | Knowledge-pack manifests, registry and install.              |
| `kgpacks-agent`      | `@kgpacks/agent`     | Graph-RAG agent over the Copilot SDK / RustyClawd.           |
| `kgpacks-query`      | `@kgpacks/query`     | Hybrid retrieval, reranking and cypher-RAG.                   |
| `kgpacks-ingestion`  | `@kgpacks/ingestion` | Fetch / chunk / extract / embed ingestion pipeline.          |
| `kgpacks-eval`       | `@kgpacks/eval`      | Evaluation harness (baselines, judge, metrics).              |
| `kgpacks-mcp`        | `@kgpacks/mcp`       | Model Context Protocol server exposing pack queries.         |
| `kgpacks-backend`    | `@kgpacks/backend`   | HTTP API surface (query / SSE).                              |
| `kgpacks-cli`        | `@kgpacks/cli`       | Command-line interface (`kgpacks` binary).                   |

The reference's `apps/frontend` (React SPA) and `parity/` (golden-fixture diff
harness against the Python/TS oracle) are tracked for a later milestone.

### Upstream dependencies

The workspace declares the Simard Rust stack as `[workspace.dependencies]` so member
crates opt in (`<dep>.workspace = true`) as real wiring lands. Pins match the Simard
workspace. As of M2, `kgpacks-db` consumes `lbug`; the others remain stubs.

| Dependency                    | Source                                  | Used by                           |
| ----------------------------- | --------------------------------------- | --------------------------------- |
| `lbug = 0.15.3`               | crates.io                               | `kgpacks-db` graph (vector/FTS M4)|
| `amplihack-memory`            | `rysweet/amplihack-memory-lib` (git)    | `kgpacks-db` graph-store helpers  |
| `rustyclawd-core` / `-tools`  | `rysweet/RustyClawd` (git)              | `kgpacks-agent` Copilot SDK       |

> **Build requirement:** `lbug` compiles LadybugDB's bundled C++ engine from
> source, so a C++ toolchain and **CMake** must be on `PATH` (`cmake`,
> `build-essential` on Debian/Ubuntu).

## Porting roadmap (M1–M5)

- **M1 — Scaffold (this milestone).** Cargo workspace mirroring the TS packages,
  SHA-pinned CI (`build`, `test`, `fmt`, `clippy`), this roadmap, and a passing
  smoke test per crate. Crates are compiling stubs.
- **M2 — Graph store + schema parity (landed).** `kgpacks-db` wraps LadybugDB
  via `lbug` (`Database` / `Connection` / `DatabaseOptions`, bound-parameter
  Cypher, extension loading, idempotent close), and `kgpacks-packs` ports the
  manifest schema + SemVer versioning and builds/loads a pack over the graph
  store. Vector/FTS indexing is deferred to M4.
- **M3 — Ingestion + embeddings (landed).** `kgpacks-embeddings` ports the
  fixed-window chunker (chunk size/overlap, id format, and windowing at parity
  with `agent-kgpacks-ts`) and an embedding generator (a deterministic,
  retrieval-contract-preserving model standing in for the BGE transformer so CI
  stays hermetic), and `kgpacks-ingestion` ports the working-store schema,
  pluggable content sources, LLM-extraction sanitization (gated behind a
  mockable `Extractor` trait), link discovery, the claim/heartbeat/reclaim work
  queue, the per-article processor, and the `process_one` orchestrator step.
  Embeddings are generated and stored; the HNSW vector index over them is M4.
- **M4 — Retrieval parity: graph + vector + FTS (landed).** `kgpacks-query` ports
  the CORE read path of `@kgpacks/query`: `vector_retrieve` (cosine search via
  `CALL QUERY_VECTOR_INDEX`), `hybrid_retrieve` (a weighted blend of the vector,
  `LINKS_TO` graph-proximity, and title-keyword/full-text signals), the
  `PackRetriever` facade (lazy `vector`/`fts` extension loading + mode dispatch),
  and the standalone read-only `validate_cypher` guard. The ENHANCEMENTS layer
  (graph reranker, cross-encoder, few-shot, Cypher-RAG, multi-document synthesis)
  and `retrieveAndSynthesize` are agent-tied and land with M5.
- **M5 — Graph-RAG agent + CLI parity (this milestone).** `kgpacks-agent` ports
  `@kgpacks/agent`: the `CopilotAgent` (answer synthesis, query expansion,
  multi-query, seed-article identification), the token/usage accountant, robust
  fenced-JSON extraction, the prompt builders, and the fail-closed error model —
  all behind an injectable `Transport` seam, with the real RustyClawd /
  Copilot-SDK adapter behind the `copilot` feature. `kgpacks-query` adds the
  agent-grounded graph-RAG query `retrieve_and_synthesize` (retrieval → grounded
  synthesis), and `kgpacks-cli` surfaces the flow (`query` / `ask`) plus the
  read-path `status` command (installed-pack summary) and the read-path `pack`
  subcommands (`list` / `info` / `validate`). The broader
  retrieval ENHANCEMENTS layer (cross-encoder, few-shot, Cypher-RAG, multi-doc
  synthesis), the `kgpacks-mcp` / `kgpacks-backend` HTTP surfaces, and the
  `parity/` harness remain follow-ups beyond this core flow.

## Build and test

Requires a stable Rust toolchain (`rustup`, with `rustfmt` and `clippy`) and a
C++ toolchain + **CMake** (for the bundled LadybugDB engine; on Debian/Ubuntu:
`sudo apt-get install -y cmake build-essential`).

```bash
cargo build --workspace --all-targets   # compile every crate and target
cargo test  --workspace                 # run the smoke tests
cargo fmt   --all -- --check            # formatting gate (matches CI)
cargo clippy --workspace --all-targets -- -D warnings  # lint gate (matches CI)
```

Run the CLI:

```bash
# Smoke-test the pipeline shape (M1 stub):
cargo run --bin kgpacks -- demo
# ingested 1 chunk(s); pack=demo@0.1.0; score=1

# Build a CVE pack from a JSON corpus (resumable + pipelined; WS6):
cargo run --bin kgpacks -- build cve --corpus ./cve-corpus.json --out ./packs/cve \
  --batch 500 --with-entity-relations
# If interrupted, re-run the SAME command with --resume to continue from the
# last committed batch instead of rebuilding from scratch:
cargo run --bin kgpacks -- build cve --corpus ./cve-corpus.json --out ./packs/cve \
  --batch 500 --with-entity-relations --resume

# Ranked retrieval over a built pack, as JSON (uses the default packs dir):
cargo run --bin kgpacks -- query rust-expert "what is ownership?" -k 5

# Graph-RAG: retrieve, then synthesize a grounded answer (needs the `copilot`
# feature to reach the real Copilot backend):
cargo run --bin kgpacks --features copilot -- ask rust-expert "what is ownership?"

# Verify a signed release index, then pull it (WS7). Fails closed on a
# tampered/untrusted signature; `--require-signature` rejects an unsigned index;
# `--no-verify` skips the check:
cargo run --bin kgpacks -- pack pull rust-expert --require-signature
```

### Packs directory

The CLI and the MCP server read installed packs from the same directory, so a
pack installed by one is found by the other. It is resolved with this precedence
(highest first):

1. the `--packs-dir <dir>` flag (CLI only);
2. the `KGPACKS_PACKS_DIR` environment variable;
3. the default: `$XDG_DATA_HOME/kgpacks` when `XDG_DATA_HOME` is set (non-empty),
   otherwise `~/.local/share/kgpacks`.

Empty or whitespace-only overrides (both the flag and the env var) are treated
as unset and fall through to the next level. The default directory is created on
first use.

```bash
# Default location (~/.local/share/kgpacks or $XDG_DATA_HOME/kgpacks):
cargo run --bin kgpacks -- query rust-expert "what is ownership?"

# Override for one process via the environment:
KGPACKS_PACKS_DIR=/srv/packs cargo run --bin kgpacks -- query rust-expert "what is ownership?"

# Override explicitly with the flag (wins over the environment):
cargo run --bin kgpacks -- --packs-dir ./packs query rust-expert "what is ownership?"
```

CI ([`.github/workflows/ci.yml`](.github/workflows/ci.yml)) runs the four default gates
(build / test / fmt / clippy) plus a dedicated `--features copilot` build+clippy step
(so the real RustyClawd transport stays compiled and linted), with all GitHub Actions
pinned to commit SHAs.

## Graph store + packs (M2)

`kgpacks-db` exposes a thin, safe wrapper over LadybugDB; parameters are always
**bound** by the driver (never interpolated into the query text):

```rust
use kgpacks_db::{Database, Value};

let db = Database::in_memory()?;
let conn = db.connect()?;
conn.run("CREATE NODE TABLE Doc(id INT64, title STRING, PRIMARY KEY(id))")?;
conn.run_params(
    "CREATE (:Doc {id: $id, title: $title})",
    vec![("id", Value::Int64(1)), ("title", Value::String("alpha".into()))],
)?;
let rows = conn.run("MATCH (d:Doc) RETURN d.title AS title")?;
```

`kgpacks-packs` builds a pack (manifest + LadybugDB graph store) and loads it back:

```rust
use kgpacks_packs::{build_pack, load_pack, Article, PackContent, PackManifest};

let manifest = PackManifest::new("rust-expert", "1.0.0");
let content = PackContent {
    articles: vec![Article {
        title: "Rust".into(),
        category: "Programming".into(),
        word_count: 1200,
        expansion_depth: 1,
    }],
    ..PackContent::default()
};

build_pack("/tmp/rust-expert", &manifest, &content)?;
let loaded = load_pack("/tmp/rust-expert")?;
assert_eq!(loaded.graph_stats()?["articles"], 1.0);
```

`validate_manifest` (the single schema gate behind `load_manifest` /
`load_manifest_from_dir` / `build_pack` / `save_manifest`) is at full parity with
the reference `validateManifest`: beyond `name`/`version`/`description` and the
optional `graph_stats`/`eval_scores` blocks, it validates the optional build
`provenance` block (`corpus` / `embedding` / `build` sections — declared string
fields must be strings, `embedding.dimensions` must be a non-negative finite
number) and deep-sanitizes it against prototype-pollution keys, mirroring
`packages/packs/src/manifest.ts`. A manifest carrying a malformed `provenance`
block is therefore rejected (and such a pack is skipped by `registry::list_packs`
/ the `status` command) exactly as the reference rejects it. Cross-checked by
`qa/manifest-provenance-parity`, which diffs the Rust `status` listing against a
live-run JS reference oracle that mirrors the TypeScript `validateManifest`
semantics over the same fixtures.

### Multi-part release + linear-scaling loader (WS5)

Packs larger than
[`MAX_SINGLE_ARTIFACT_BYTES`](crates/kgpacks-packs/src/release.rs) (2 GiB) are
published as an ordered set of fixed-size parts. `plan_multipart_release` is the
release tool's split/accounting step run **dry** (it computes the index a
`pack pull` re-verifies; it publishes nothing): every non-final part is exactly
`part_size`, `sum(parts.bytes) == total_bytes`, each part carries the SHA-256 of
its own bytes, and the index carries the SHA-256 of the whole artifact. Size
accounting (`part_accounting`) is pure `u64` arithmetic, so the >2 GiB path is
covered without materializing gigabytes. Hashing uses a self-contained SHA-256
([`kgpacks-packs::sha256`](crates/kgpacks-packs/src/sha256.rs)) — no external
crypto dependency.

```rust
use kgpacks_packs::{plan_multipart_release, part_accounting, requires_multipart};

// Split a (small, here) artifact into 1 KiB parts and hash each part + the whole.
let index = plan_multipart_release(&artifact_bytes, 1024)?;
assert_eq!(index.parts.iter().map(|p| p.bytes).sum::<u64>(), index.total_bytes);

// >2 GiB accounting without allocating the bytes.
assert!(requires_multipart(3 * 1024 * 1024 * 1024));
let acct = part_accounting(3 * 1024 * 1024 * 1024, 64 * 1024 * 1024)?; // 48 parts
```

The graph loader is also guarded to scale **linearly**: `plan_load_statements`
separates the load *plan* from execution so a structural test can assert that
loading 2N records issues at most ~3× the statements of N, and that every
edge-creation statement uses PK-indexed single-`MATCH` clauses rather than the
O(N²) comma two-pattern `MATCH (a {..}), (b {..})`. These guards run in CI as
part of `cargo test --workspace` (`kgpacks-packs/tests/multipart_release.rs`,
`kgpacks-packs/tests/linear_scaling_guard.rs`, and
`kgpacks-ingestion/tests/linear_scaling_guard.rs`).

## Resumable, pipelined CVE pack build (WS6)

For large CVE packs, `kgpacks-packs` provides a build path that is both
**resumable** and **pipelined** ([`build_cve_pack`](crates/kgpacks-packs/src/cve_build.rs)),
alongside the one-shot [`build_pack`](crates/kgpacks-packs/src/pack.rs):

- **Checkpoint / resume.** After every committed batch the builder writes a
  sidecar next to the graph store — `graph.lbug.build-checkpoint.json` — recording
  the last committed batch, the corpus offset, running counts and a stable
  `params_hash` (a SHA-256 over the output-affecting inputs `src`, `year`,
  `limit`, `batch`, `model`, `with_entity_relations`, independent of key order).
  Re-running with resume enabled continues from the checkpoint (reopening the
  store, skipping schema creation, rebuilding its de-dup set from the store, and
  loading only the remaining records). If any parameter changed — so the recorded
  `params_hash` no longer matches — the build restarts cleanly instead. The
  sidecar is removed on a clean finish, so a completed pack carries no resume
  state.
- **Pipelined `embed || load`.** Embedding (CPU-bound, parallelizable) runs on a
  worker thread while the database load/index (serial, single-writer) runs on the
  caller's thread, connected by a **bounded** channel that overlaps the two
  stages while capping how many embedded batches are held in memory.

The corpus is consumed only through the
[`CorpusSource`](crates/kgpacks-packs/src/corpus.rs) seam, so the external
CVE-corpus fetch (issue #25) can plug in a live source without changing the
builder. Until then, `FixtureCorpus` loads a corpus from a JSON array of
records (`id`, `description`, optional `published_year`, `entities`, `relations`).

### CLI

```bash
# Fresh build (fails if the pack already exists, so a finished pack is never
# clobbered):
kgpacks build cve --corpus ./cve-corpus.json --out ./packs/cve --batch 500

# Resume an interrupted build from its checkpoint (same parameters), or start
# clean if the parameters changed:
kgpacks build cve --corpus ./cve-corpus.json --out ./packs/cve --batch 500 --resume
```

`build <pack>` flags:

| Flag | Default | Meaning |
| --- | --- | --- |
| `--corpus <file.json>` | *(required)* | CVE corpus JSON (the #25 seam). |
| `--out <dir>` | `<packs-dir>/<pack>` | Output pack directory. |
| `--batch <n>` | `64` | Records loaded (and checkpointed) per batch. |
| `--limit <n>` | *(all)* | Cap on the number of corpus records considered (a prefix). |
| `--year <y>` | *(none)* | Load only records whose `published_year` equals `y`. |
| `--with-entity-relations` | off | Materialize `ENTITY_RELATION` edges. |
| `--queue <n>` | `2` | Bound on embedded batches buffered between embed and load. |
| `--pack-version <semver>` | `1.0.0` | Version written to the pack manifest. |
| `--resume` | off | Resume from a matching checkpoint (else clean restart). |

The command prints a JSON report (`paramsHash`, `resumed`, `resumedFromBatch`,
`batchesCommitted`, `counts`). The checkpoint file lives at
`<out>/graph.lbug.build-checkpoint.json` while a build is in progress and is
removed on a clean finish.

## Ingestion pipeline (M3)

`kgpacks-ingestion` drives the **build-pack → ingest** flow over the working
store: open a store, seed it, and process articles fetched from a
[`ContentSource`] through chunking, embedding, optional LLM extraction, and the
expansion state machine.

```rust
use kgpacks_embeddings::Embedder;
use kgpacks_ingestion::{
    ArticleInfo, ExpansionConfig, MapContentSource, Orchestrator, Article,
};

// An in-memory content source (real Wikipedia/web sources land later).
let source = MapContentSource::new().with_article(Article {
    title: "Rust".into(),
    content: "Rust is a systems language.\n\n## Features\nOwnership and borrowing.".into(),
    links: vec!["Cargo".into()],
    categories: vec!["Programming languages".into()],
    source_type: "memory".into(),
    ..Article::default()
});

let orchestrator = Orchestrator::in_memory(ExpansionConfig::default())?;
orchestrator.initialize_seeds(&["Rust".into()], "Programming")?;

let embedder = Embedder::bge(); // 768-d, deterministic (matches the store schema)
let conn = orchestrator.connect()?;
let (_title, success, _err) = orchestrator.process_claimed(
    &conn,
    &ArticleInfo::new("Rust", 0).with_category("Programming"),
    &source,
    &embedder,
    None, // or Some(&extractor) to also load entities/facts/relationships
)?;
assert!(success);
```

The crate is a one-to-one port of the reference ingestion modules; each maps to
a Rust module proven by a mirroring parity test:

| Reference (`agent-kgpacks`)              | Rust module                              | Parity test                          |
| ---------------------------------------- | ---------------------------------------- | ------------------------------------ |
| `ingestion/src/chunking.ts` (TS parity)  | `kgpacks-embeddings::chunker`            | `kgpacks-embeddings/tests/chunker.rs` |
| `embeddings/generator.py`                | `kgpacks-embeddings` (`Embedder`)        | `kgpacks-embeddings/tests/generator.rs` |
| `extraction/llm_extractor.py`            | `kgpacks-ingestion::extraction`          | `kgpacks-ingestion/tests/extraction.rs` |
| `expansion/link_discovery.py`            | `kgpacks-ingestion::link_discovery`      | `kgpacks-ingestion/tests/link_discovery.rs` |
| `expansion/work_queue.py`                | `kgpacks-ingestion::work_queue`          | `kgpacks-ingestion/tests/work_queue.rs` |
| `expansion/orchestrator.py`              | `kgpacks-ingestion::orchestrator`        | `kgpacks-ingestion/tests/orchestrator.rs` |
| `expansion/processor.py` + `database/loader.py` | `kgpacks-ingestion::processor`    | `kgpacks-ingestion/tests/processor.rs` |
| `schema/ryugraph_schema.py`              | `kgpacks-ingestion::schema`              | exercised by the above               |

LLM extraction is gated behind the `Extractor` trait (`MockExtractor` /
`JsonExtractor` for tests), and the embedder is a deterministic stand-in for the
BGE transformer, so the whole suite runs offline. Two documented working-store
deviations from the reference: timestamps are `INT64` epoch-millis (not
`TIMESTAMP`), and the embedding columns are populated but the HNSW vector index
over them is deferred to M4.

## Retrieval (M4)

`kgpacks-query` turns a natural-language query into a ranked list of section
hits over a built pack. A [`PackRetriever`] binds a [`kgpacks-db`] connection, an
embedder, and a pack schema (defaults `Section` / `embedding_idx`), then
dispatches `retrieve` to one of two modes:

- **`vector`** (default) — embed the query, run cosine search via
  `CALL QUERY_VECTOR_INDEX`, score each hit `clamp(1 - distance, 0, 1)`, nearest
  first.
- **`hybrid`** — blend three weighted signals into one score per node (reference
  `hybrid_retrieve` defaults `vector 0.5 / graph 0.3 / keyword 0.2`):
  1. **vector** cosine similarity,
  2. **graph** `LINKS_TO` proximity from the first few scored nodes
     (`+ graph_weight * 0.5` per neighbor),
  3. **keyword / full-text** title `CONTAINS` matches for the leading query
     terms (`+ keyword_weight * 0.7` per match).

```rust
use kgpacks_db::{Database, Value};
use kgpacks_query::{PackRetriever, RetrieveMode, RetrieveOptions};

let db = Database::in_memory()?;
let conn = db.connect()?;

// A pack is a Section table with a FLOAT[768] embedding indexed for cosine
// search, plus LINKS_TO edges for the graph signal (built by ingestion).
conn.load_extension("vector")?;
conn.run(
    "CREATE NODE TABLE Section(id STRING, title STRING, content STRING, \
     embedding FLOAT[768], PRIMARY KEY(id))",
)?;
conn.run("CREATE REL TABLE LINKS_TO(FROM Section TO Section)")?;
// … insert Section rows with embeddings …
conn.run("CALL CREATE_VECTOR_INDEX('Section', 'embedding_idx', 'embedding', metric := 'cosine')")?;

// Vector retrieval (BGE-parity embedder by default).
let retriever = PackRetriever::new(&conn);
let top = retriever.retrieve("how do plants make energy?", &RetrieveOptions {
    k: Some(5),
    ..Default::default()
})?;

// Hybrid retrieval (vector + graph + full-text).
let blended = retriever.retrieve("photosynthesis", &RetrieveOptions {
    k: Some(5),
    mode: RetrieveMode::Hybrid,
    weights: None, // reference defaults
})?;
```

The crate is a one-to-one port of the reference CORE retrieval modules; each maps
to a Rust module proven by a mirroring parity test:

| Reference (`@kgpacks/query`)         | Rust module                                  | Parity test                       |
| ------------------------------------ | -------------------------------------------- | --------------------------------- |
| `query/src/vector.ts`                | `kgpacks-query::vector` (`vector_retrieve`)  | `tests/vector.rs`                 |
| `query/src/hybrid.ts`                | `kgpacks-query::hybrid` (`hybrid_retrieve`)  | `tests/hybrid.rs`, `tests/combined.rs` |
| `query/src/retriever.ts` (CORE)      | `kgpacks-query::retriever` (`PackRetriever`) | `tests/vector.rs`, `tests/hybrid.rs` |
| `query/src/cypher-safety.ts`         | `kgpacks-query::cypher_safety` (`validate_cypher`) | `tests/cypher_safety.rs`    |
| `query/src/row.ts`                   | `kgpacks-query::row`                          | `tests/row.rs`                    |
| `query/src/{constants,types,errors}.ts` | `kgpacks-query::{constants,types,errors}` | `tests/surface.rs`                |

Retrieval is driven through a deterministic injected embedder in the parity
suite (known one-hot/graded vectors), so cosine similarities are exact and the
only variability under test is the scoring formula itself — the same approach the
reference `hybrid.test.ts` uses. This keeps the suite **offline and hermetic**
while reproducing the reference arithmetic exactly (e.g. the hybrid worked
example scores `{1: 0.64, 2: 0.15}`). The `id` coercion handles both the
reference `INT64` fixtures and the RS pack's `STRING` `Section.id`. Two parity
notes carried from the reference: the keyword signal is a title `CONTAINS` match
(not an FTS-index procedure), and `validate_cypher` is exported as a standalone
guard — the CORE read path itself never routes user text into Cypher (it runs
fixed, parameter-bound vector/graph queries).

## Graph-RAG agent + CLI (M5)

`kgpacks-agent` ports `@kgpacks/agent`: a `CopilotAgent` wrapping a Copilot
session through an injectable `Transport` seam, exposing the four ported
operations plus usage accounting.

```rust
use kgpacks_agent::{CopilotAgent, CopilotAgentOptions, SynthesisRequest, ContextChunk};

let mut agent = CopilotAgent::with_transport(transport, CopilotAgentOptions::default());
agent.start()?;
let result = agent.synthesize_answer(&SynthesisRequest {
    question: "What is HNSW?".into(),
    context: vec![ContextChunk::new("doc:1", "HNSW is a navigable small-world graph.")],
    ..SynthesisRequest::default()
})?;
assert_eq!(result.metadata.cited_ids, ["doc:1"]); // citations derived from the answer
agent.stop()?;
```

The agent **fails closed** (returns shape-checked data or an `AgentError`), pins
the held-constant model, bounds context to cap cost/DoS surface, and redacts
BYOK secrets from surfaced transport errors. The real Copilot adapter
(`copilot_transport`, behind the `copilot` feature) wires the seam to
`rustyclawd-core`'s Copilot backend; every unit test injects a mock instead, so
the suite is fully offline.

`kgpacks-query::retrieve_and_synthesize` is the **graph-RAG query**: it runs M4
retrieval, hands the ranked sections to the agent as citation-tagged context,
and returns the grounded answer plus its supporting hits. `kgpacks-cli` surfaces
it (`query` → ranked JSON, `ask` → grounded answer JSON). The read-path `status`
command reports the resolved packs directory and the installed packs as JSON
(`{ packsDir, count, packs: [{ name, version, dbPresent }] }`, sorted by name),
backed by `kgpacks-packs::registry::list_packs`. The read-path subset of the
reference `pack` command group is also ported — `pack list`
(`[{ name, version, description }]`, sorted by name), `pack info <pack>` (the
pack's full manifest), and `pack validate <pack>` (`{ valid, name, version }`) —
over the same `kgpacks-packs` registry/manifest APIs. The offline release
planner is also ported — `pack release-plan <pack> [--tag <t>] [--model <id>]
[--corpus-commit <sha>] [--corpus-date <date>]` prints the network-free plan
(`{ name, tag, version, model, provenance, publishTargets, indexFilename }`) for
publishing a pack. The write/network `pack` subcommands (`install`/`pull`/`remove`),
the byte-level release packaging + `gh` upload behind `release-plan`, and the
ingestion/eval verbs (`create`/`update`/`eval`) remain follow-ups (issue #13).

### Versioned release tags + provenance (WS3)

A pack is published to an **immutable dated release tag** `<name>-YYYY.MM[.N]`
(e.g. `cve-2025.06`) whose SemVer version is derived **unpadded** — the tag's
zero-padded month becomes a leading-zero-free numeric core, since SemVer 2.0
forbids leading zeros: `pack_version_from_release_tag("cve-2025.06")` →
`"2025.6.0"`, `"cve-2025.06.1"` → `"2025.6.1"`. Undated tags (the stable `packs`
latest-pointer, `cve`, `cve-latest`, the empty string) and out-of-range months
are rejected. Every dated release also moves the stable `packs` latest-pointer to
the same assets (`publish_targets("cve-2025.06")` → `["cve-2025.06", "packs"]`),
so the `pack pull` UX — which defaults to `packs` — always resolves the newest
version (`latest_release_tag` resolves the highest dated tag by SemVer
precedence). The pack `manifest.json` build `provenance` block
(`corpus`/`embedding`/`build`) is mirrored into the `<name>.pack-release.json`
release index (`kgpacks-packs::release::PackReleaseIndex`), filling gaps from CLI
overrides and a release-time `build.date`, so the manifest and the release index
can be cross-checked. This ports `scripts/release-pack.mjs` +
`packages/packs/src/versioning.ts` `packVersionFromReleaseTag`; the pure, offline
half (version/provenance/publish-target/latest resolution + the index model) is
implemented and tested here, while the byte-level multipart `tar.gz` packaging +
`gh` upload remain follow-ups (issue #13), like the write/network `pull` path.

Each reference module maps to a Rust module proven by a mirroring parity test:

| Reference (`@kgpacks/agent` / `@kgpacks/cli`) | Rust module                                  | Parity test                          |
| --------------------------------------------- | -------------------------------------------- | ------------------------------------ |
| `agent/src/copilot-agent.ts`                  | `kgpacks-agent::copilot_agent`               | `agent/tests/copilot_agent.rs`       |
| `agent/src/json.ts`                           | `kgpacks-agent::json`                        | `agent/tests/json.rs`                |
| `agent/src/usage.ts`                          | `kgpacks-agent::usage`                       | `agent/tests/usage.rs`               |
| `agent/src/transport.ts` (+ `types.ts`)       | `kgpacks-agent::{transport,types}`           | `agent/tests/transport_contract.rs`  |
| `agent/src/{prompts,errors,constants}.ts`     | `kgpacks-agent::{prompts,errors,constants}`  | exercised by the above               |
| `query/src/retriever.ts` `retrieveAndSynthesize` | `kgpacks-query::synthesis`                | `query/tests/synthesis.rs`           |
| `cli/src/commands/query.ts` (+ `ask` flow)    | `kgpacks-cli` (`query` / `ask`)              | `cli/tests/e2e.rs`, unit tests       |
| `cli/src/commands/status.ts` (+ `packs/registry.ts` `listPacks`) | `kgpacks-cli` (`status`) + `kgpacks-packs::registry::list_packs` | `cli/tests/e2e.rs`, `qa/status-parity/` |
| `cli/src/commands/pack.ts` (read-path: `list`/`info`/`validate`) | `kgpacks-cli` (`pack list`/`info`/`validate`) over `kgpacks-packs::{registry,manifest}` | `cli/tests/e2e.rs`, unit tests, `qa/pack-parity/` |
| `scripts/release-pack.mjs` + `versioning.ts` `packVersionFromReleaseTag` | `kgpacks-packs::release` + `kgpacks-cli` (`pack release-plan`) | `packs/tests/release.rs`, `packs/tests/versioning.rs`, unit tests, `qa/release-parity/` |

The agent parity suite mirrors the reference's `agent/test/*` structurally
(valid shapes, fence-stripping, citation derivation, usage accounting,
lifecycle, fail-closed errors) against a mock transport, and the CLI `e2e` test
drives the full `build pack → vector retrieval → graph-RAG query` flow through
the actual command surface offline. The `status` command additionally ships a
cross-implementation parity harness (`qa/status-parity/`, driven by the
`status-ts-parity` qa-team scenario) that diffs the Rust payload against a live
`agent-kgpacks-ts` reference oracle; the read-path `pack` subcommands ship an
equivalent harness (`qa/pack-parity/`, driven by the `pack-ts-parity` scenario)
that diffs `pack list`/`info`/`validate` against the same live reference. Scope
notes: the real transport is gated
behind the non-default `copilot` feature (compiled + linted in a dedicated CI
step) so the default gates stay lean and hermetic; the remaining write/eval CLI
path (`create`/`update`, `research-sources`, the write/network `pack`
subcommands, and `eval`; issue #13), the retrieval ENHANCEMENTS layer, the
MCP/backend HTTP surfaces, and the `parity/` harness remain follow-ups beyond
this core flow.

## Quality audit

The repository is maintained under a **Ten-Wave Quality Audit** — a repeatable,
agent-driven process that runs ten `SEEK → VALIDATE → FIX` waves across
correctness, memory safety, error handling, idiomatic Rust, test
coverage/quality, and M1–M5 parity with the TypeScript reference. Every fix PR is
gated behind CI **and** an explicit proxy review from the `crusty-old-engineer`
reviewer. See [`docs/quality-audit/`](docs/quality-audit/README.md) for the
usage, reference, configuration, and worked examples.

## Release-index signing (WS7)

Beyond the sha256 **integrity** already carried in the `<name>.pack-release.json`
release index, WS7 ([#22](https://github.com/rysweet/agent-kgpacks-rs/issues/22))
adds Ed25519 **authenticity**: the index is signed with a release private key
(a CI secret, never committed), and `pack pull` verifies that signature before
trusting the index.

The primitives live in [`kgpacks-packs::signing`](crates/kgpacks-packs/src/signing.rs)
and are deliberately **format-agnostic** — they sign and verify the *raw
serialized bytes* of the index and never parse it, so they compose additively
over the WS3 ([#18](https://github.com/rysweet/agent-kgpacks-rs/issues/18))
release-index schema whether or not it has landed:

- `SigningKeyPair` — pure-Rust Ed25519 (`ed25519-dalek`) keypair generation
  (OS-CSPRNG seeded), detached signing over the raw index bytes, and the
  `<name>.pack-release.json.sig` sidecar (`{algorithm, signature, public_key}`,
  base64).
- `verify_pack_index_signature(index_bytes, sig, trusted_pubkey) -> bool` —
  **verify-before-parse** over the raw bytes; rejects tampered bytes, wrong-length
  inputs, and signatures from untrusted keys (uses `verify_strict`, never panics).
- `signature_plan(SignatureInputs{present, valid, require_signature, no_verify})`
  → `Verify | Fail | Warn | Skip` — the pull-time policy: present+valid ⇒
  **Verify**; present+invalid ⇒ **Fail** (fail-closed); absent ⇒ **Warn**
  (integrity-only) unless `--require-signature` ⇒ **Fail**; `--no-verify` ⇒
  **Skip**. `--require-signature` together with `--no-verify` is a usage error
  (`validate_signature_flags`).

The **trusted** public key is committed at
[`crates/kgpacks-packs/keys/pack-release-signing.pub`](crates/kgpacks-packs/keys/pack-release-signing.pub)
(base64 raw 32 bytes); `pack pull` verifies against it by default, overridable
with `--trusted-key <base64>` or `KGPACKS_TRUSTED_RELEASE_KEY`. Trust is anchored
on that committed key — **not** on the `public_key` a sidecar happens to carry.

```bash
# Verify against the committed trusted key (default), requiring a signature:
kgpacks --packs-dir ./packs pack pull rust-expert --require-signature
# Skip verification (integrity-only):
kgpacks --packs-dir ./packs pack pull rust-expert --no-verify
```

| Behavior (issue #22)                                   | Rust surface                                          | Test                                                    |
| ------------------------------------------------------ | ----------------------------------------------------- | ------------------------------------------------------- |
| Keypair gen + detached sign over raw index bytes       | `signing::SigningKeyPair`                             | `packs/tests/signing.rs`, `signing` unit tests          |
| `verify_pack_index_signature` (tamper / wrong-key)     | `signing::verify_pack_index_signature`               | `packs/tests/signing.rs`, `signing` unit tests          |
| Pull-time policy + mutually-exclusive flags            | `signing::{signature_plan, validate_signature_flags}` | `signing` unit tests, `cli/tests/pack_pull.rs`          |
| `pack pull` verify path (fail-closed)                  | `kgpacks-cli` (`pack pull`)                           | `cli/tests/pack_pull.rs`                                |

## License

[MIT](LICENSE).
