# agent-kgpacks-rs

A Rust port of [`agent-kgpacks`](https://github.com/rysweet/agent-kgpacks-ts) — a
knowledge-pack platform that builds a **LadybugDB** graph from source documents and
answers questions with a **graph-RAG agent** over the GitHub Copilot SDK.

The executable specification for this port is the TypeScript reference
[`rysweet/agent-kgpacks-ts`](https://github.com/rysweet/agent-kgpacks-ts) (a pnpm
workspace). This repository mirrors that module decomposition as a **Cargo
workspace** and reuses the Simard Rust stack (`lbug` for the graph + vector/FTS
engine, RustyClawd / Copilot SDK for the agent).

> **Status:** M2 — graph store + schema parity. `kgpacks-db` is wired to
> LadybugDB (`lbug`) and `kgpacks-packs` builds/loads a pack over the graph
> store. The remaining crates are compiling stubs. See the
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
- **M3 — Ingestion + embeddings.** Implement `kgpacks-ingestion` (fetch, chunk,
  extract, expand) and real embeddings in `kgpacks-embeddings`.
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

## License

[MIT](LICENSE).
