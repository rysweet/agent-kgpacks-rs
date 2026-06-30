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
> prints ranked retrieval and `ask` prints a grounded, citation-bearing answer.
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
  sentence-aware chunker and an embedding generator (a deterministic,
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
  synthesis), and `kgpacks-cli` surfaces the flow (`query` / `ask`). The broader
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

# Ranked retrieval over a built pack, as JSON:
cargo run --bin kgpacks -- --packs-dir ./packs query rust-expert "what is ownership?" -k 5

# Graph-RAG: retrieve, then synthesize a grounded answer (needs the `copilot`
# feature to reach the real Copilot backend):
cargo run --bin kgpacks --features copilot -- --packs-dir ./packs ask rust-expert "what is ownership?"
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
| `embeddings/chunker.py`                  | `kgpacks-embeddings::chunker`            | `kgpacks-embeddings/tests/chunker.rs` |
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
it (`query` → ranked JSON, `ask` → grounded answer JSON).

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

The agent parity suite mirrors the reference's `agent/test/*` structurally
(valid shapes, fence-stripping, citation derivation, usage accounting,
lifecycle, fail-closed errors) against a mock transport, and the CLI `e2e` test
drives the full `build pack → vector retrieval → graph-RAG query` flow through
the actual command surface offline. Scope notes: the real transport is gated
behind the non-default `copilot` feature (compiled + linted in a dedicated CI
step) so the default gates stay lean and hermetic; the retrieval ENHANCEMENTS
layer, the MCP/backend HTTP surfaces, and the `parity/` harness remain
follow-ups beyond this core flow.

## License

[MIT](LICENSE).
