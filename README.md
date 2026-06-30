# agent-kgpacks-rs

A Rust port of [`agent-kgpacks`](https://github.com/rysweet/agent-kgpacks-ts) — a
knowledge-pack platform that builds a **LadybugDB** graph from source documents and
answers questions with a **graph-RAG agent** over the GitHub Copilot SDK.

The executable specification for this port is the TypeScript reference
[`rysweet/agent-kgpacks-ts`](https://github.com/rysweet/agent-kgpacks-ts) (a pnpm
workspace). This repository mirrors that module decomposition as a **Cargo
workspace** and reuses the Simard Rust stack (`lbug` for the graph + vector/FTS
engine, RustyClawd / Copilot SDK for the agent).

> **Status:** M3 — ingestion pipeline parity. On top of the M2 graph store,
> `kgpacks-ingestion` ports the fetch → chunk → extract → embed → load pipeline
> (content sources, the expansion state machine, link discovery, and LLM
> extraction sanitization) and `kgpacks-embeddings` ports chunking and embedding
> generation. The retrieval/agent crates remain compiling stubs. See the
> [roadmap](#porting-roadmap-m1m5) for what lands when.

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
- **M4 — Hybrid retrieval.** Implement `kgpacks-query` vector + FTS retrieval,
  cross-encoder reranking and safe Cypher-RAG generation.
- **M5 — Agent + surfaces.** Wire `kgpacks-agent` to the Copilot SDK via RustyClawd
  and complete the `kgpacks-cli`, `kgpacks-mcp` and `kgpacks-backend` surfaces, plus
  the `parity/` harness against the reference.

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

Run the CLI (a stub `demo` wires one piece of every crate together):

```bash
cargo run --bin kgpacks -- demo
# ingested 1 chunk(s); pack=demo@0.1.0; score=1
```

CI ([`.github/workflows/ci.yml`](.github/workflows/ci.yml)) runs the same four gates
on every push and pull request, with all GitHub Actions pinned to commit SHAs.

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

## License

[MIT](LICENSE).
