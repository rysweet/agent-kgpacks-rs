# agent-kgpacks-rs

A Rust port of [`agent-kgpacks`](https://github.com/rysweet/agent-kgpacks-ts) — a
knowledge-pack platform that builds a **LadybugDB** graph from source documents and
answers questions with a **graph-RAG agent** over the GitHub Copilot SDK.

The executable specification for this port is the TypeScript reference
[`rysweet/agent-kgpacks-ts`](https://github.com/rysweet/agent-kgpacks-ts) (a pnpm
workspace). This repository mirrors that module decomposition as a **Cargo
workspace** and reuses the Simard Rust stack (`lbug` for the graph + vector/FTS
engine, RustyClawd / Copilot SDK for the agent).

> **Status:** M1 — workspace scaffold. Crates are stubs that compile, test green,
> and define the public surface each milestone fills in. See the
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
workspace; M1 ships stubs and does not yet consume them.

| Dependency                    | Source                                  | Used by (planned)                 |
| ----------------------------- | --------------------------------------- | --------------------------------- |
| `lbug = 0.15.3`               | crates.io                               | `kgpacks-db` graph + vector + FTS |
| `amplihack-memory`            | `rysweet/amplihack-memory-lib` (git)    | `kgpacks-db` graph-store helpers  |
| `rustyclawd-core` / `-tools`  | `rysweet/RustyClawd` (git)              | `kgpacks-agent` Copilot SDK       |

## Porting roadmap (M1–M5)

- **M1 — Scaffold (this milestone).** Cargo workspace mirroring the TS packages,
  SHA-pinned CI (`build`, `test`, `fmt`, `clippy`), this roadmap, and a passing
  smoke test per crate. Crates are compiling stubs.
- **M2 — Graph store + schema parity.** Wire `kgpacks-db` to LadybugDB via `lbug`;
  port the node/edge schema and migrations from `@kgpacks/db`.
- **M3 — Ingestion + embeddings.** Implement `kgpacks-ingestion` (fetch, chunk,
  extract, expand) and real embeddings in `kgpacks-embeddings`.
- **M4 — Hybrid retrieval.** Implement `kgpacks-query` vector + FTS retrieval,
  cross-encoder reranking and safe Cypher-RAG generation.
- **M5 — Agent + surfaces.** Wire `kgpacks-agent` to the Copilot SDK via RustyClawd
  and complete the `kgpacks-cli`, `kgpacks-mcp` and `kgpacks-backend` surfaces, plus
  the `parity/` harness against the reference.

## Build and test

Requires a stable Rust toolchain (`rustup`, with `rustfmt` and `clippy`).

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

## License

[MIT](LICENSE).
