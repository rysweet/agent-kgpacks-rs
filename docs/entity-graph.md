# Entity-graph traversal + scalable `ENTITY_RELATION` bulk load (WS8)

Rust port of the `agent-kgpacks-ts` WS8 workstream. Two capabilities land here, so
`ENTITY_RELATION` becomes a real, scalable feature:

1. **Entity-graph traversal** — a transport-agnostic query over the
   `Entity` / `HAS_ENTITY` / `ENTITY_RELATION` graph, exposed both as a library
   call ([`kgpacks_query::entity_graph`]) and via the backend API
   (`GET /api/v1/graph/entities`).
2. **Scalable bulk `ENTITY_RELATION` load** — a batched/streamed loader
   ([`kgpacks_ingestion::bulk_create_entity_relations`]) that avoids the per-row
   round-trips (and the O(N²) comma two-pattern `MATCH`) of the naive loop, so
   `--with-entity-relations` scales to large packs.

## Entity-graph query

```rust
use kgpacks_db::Database;
use kgpacks_query::{entity_graph, EntityGraphMode, EntityGraphOptions};

let db = Database::in_memory()?;
let conn = db.connect()?;
// … a pack with Entity / HAS_ENTITY (and optionally ENTITY_RELATION) …

let graph = entity_graph(&conn, &EntityGraphOptions {
    entity: "CVE-2021-44228".into(), // seed = Entity primary key (entity_id)
    depth: Some(2),                  // 1..=3
    type_filter: None,               // restrict neighbors (depth > 0) to a type
    mode: EntityGraphMode::Auto,     // auto | co-occurrence | relation
    limit: Some(50),                 // cap total nodes AND per-expansion fan-out
})?;

for node in &graph.nodes {
    println!("{} ({}) depth={} articles={}", node.name, node.type_, node.depth, node.articles_count);
}
```

### Traversal modes

| Mode            | Two entities are linked when …                                                | Edge `weight`             | Edge `relation`     |
| --------------- | ----------------------------------------------------------------------------- | ------------------------- | ------------------- |
| `co-occurrence` | some `Article` `HAS_ENTITY` **both** (the CVE-pack default builder skips rels) | shared-article count      | `"co_occurs"`       |
| `relation`      | an explicit `Entity`→`Entity` `ENTITY_RELATION` edge connects them            | `1`                       | the stored relation |

`EntityGraphMode::Auto` resolves to `relation` when the pack has **any**
`ENTITY_RELATION` edge, else `co-occurrence`. An older pack whose schema lacks the
`ENTITY_RELATION` table resolves to `co-occurrence` (the missing-table error is
treated as "no relation edges"). The resolved mode is reported in
`result.mode`.

### Bounded, deterministic results

- **Breadth-first** expansion records the **shortest** depth at which each entity
  is reached (`0` = seed).
- Each expansion is **fan-out-capped** at `limit` (neighbors ordered by name), so
  a high-degree hub seed cannot blow up the traversal.
- Nodes are ordered **`depth ASC, then name ASC`** and the total node set is
  truncated to `limit` — a stable, transport-stable ordering.
- The seed's own type is **never** subject to `type_filter` (only neighbors are).
- The typed result carries per-node `depth` / `type` / `articles_count`, per-edge
  `source` / `target` / `weight` (and `relation`), `total_nodes` / `total_edges`,
  and an `execution_time_ms`.

### Validation

- `depth` must be an integer `1..=3` — otherwise `QueryError::InvalidArgument`.
- An unknown seed entity yields `QueryError::EntityNotFound(seed)`.

## Backend API — `GET /api/v1/graph/entities`

The backend surface ([`kgpacks_backend::graph_entities`]) validates a raw query
against the request contract, then builds the neighborhood, mapping the query
crate's typed failures onto the standard error envelope.

| Parameter | Type    | Rule                                       | Default |
| --------- | ------- | ------------------------------------------ | ------- |
| `entity`  | string  | **required**, non-empty, `<= 500` chars    | —       |
| `depth`   | integer | `1..=3`                                     | `1`     |
| `limit`   | integer | `1..=200`                                   | `50`    |
| `type`    | string  | `<= 200` chars                              | —       |
| `mode`    | enum    | `auto` \| `co-occurrence` \| `relation`     | `auto`  |

Error envelopes (`{ "error": { "code", "message", "details" }, "timestamp" }`):

- **`400 MISSING_PARAMETER`** — `entity` absent.
- **`400 INVALID_PARAMETER`** — any other validation failure (bad `depth` / `limit`
  / `mode` / over-long `entity` or `type`).
- **`404 NOT_FOUND`** — the seed entity does not exist.

```rust
use kgpacks_backend::{graph_entities, GraphEntitiesQuery};

let query = GraphEntitiesQuery::from_pairs([
    ("entity", "CVE-2021-44228"),
    ("depth", "2"),
    ("mode", "co-occurrence"),
]);
match graph_entities(&conn, &query) {
    Ok(result) => { /* serialize `result` */ }
    Err(api_error) => {
        let status = api_error.status_code;              // 400 | 404 | 500
        let body = api_error.to_envelope().to_json();    // standard envelope
    }
}
```

> The RS backend has no HTTP server yet (it lands with the rest of the transport in
> M5). `graph_entities` is the transport-agnostic handler: it takes the raw query a
> querystring would carry and returns the typed result or an `ApiError`, ready to
> bind to the HTTP router when it lands.

## Scalable bulk `ENTITY_RELATION` load

[`bulk_create_entity_relations`] loads `Entity`→`Entity` relationship edges
scalably. It **prefers** LadybugDB's `COPY ENTITY_RELATION FROM <csv>` — a single
bulk import that scales to the full corpus — and **falls back** to PK-indexed
`UNWIND … MATCH … MATCH … CREATE` batches ([`create_entity_relations_batched`])
when `COPY` is unavailable or rejects the file.

Both shapes are **non-O(N²)**: neither uses a comma two-pattern `MATCH` (which
hash-joins the growing `Entity` table against itself); the fallback point-looks-up
each endpoint by primary key with **two separate** `MATCH` clauses.

```rust
use kgpacks_ingestion::{bulk_create_entity_relations, EntityRelationRow};

let rows = vec![
    EntityRelationRow { source_id: "A|Rust".into(), target_id: "A|Cargo".into(),
                        relation: "uses".into(), context: "Rust uses Cargo".into() },
    // …
];
let created = bulk_create_entity_relations(&conn, &rows)?;
```

The caller pre-filters to rows whose **both** endpoints already exist as `Entity`
nodes: `COPY <Rel>` errors on a dangling foreign key, so a dangling row would abort
the whole import. The single-article `ArticleProcessor` load path does exactly this
(it filters each article's relationships against the entities it just created)
before delegating to the bulk loader, replacing the previous per-row
`MATCH (e1), (e2) CREATE` loop.

`ENTITY_RELATION` remains **default-skipped** in the CVE builder — it is built only
under `--with-entity-relations`. No read path depends on it: entity-graph traversal
falls back to co-occurrence when there are no relation edges.

When `--with-entity-relations` **is** set, the resumable/pipelined CVE builder
(`kgpacks_packs::cve_build`) creates each `ENTITY_RELATION` (and every `HAS_ENTITY`)
edge with the same PK-indexed shape — two separate `MATCH` clauses that point-look-up
each endpoint by primary key (`kgpacks_packs::CREATE_ENTITY_RELATION_CYPHER` and
`kgpacks_packs::CREATE_HAS_ENTITY_CYPHER`), **never** the comma two-pattern
`MATCH (s ..), (t ..)`. A missing endpoint is silently skipped by `MATCH`, so the
builder can stream relations whose endpoints span records without pre-filtering. The
[linear-scaling guard](../crates/kgpacks-packs/tests/linear_scaling_guard.rs) pins
this at both edge Cyphers' definition sites.
