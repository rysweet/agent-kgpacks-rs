//! `kgpacks-query` — entity-graph traversal.
//!
//! Rust port of `@kgpacks/query`'s `entity-graph.ts`. A transport-agnostic
//! entity-neighborhood query over the `Entity` / `HAS_ENTITY` / `ENTITY_RELATION`
//! graph, reused by the backend `/api/v1/graph/entities` API (and, later, the
//! MCP/CLI). Two traversal modes:
//!
//!   * **co-occurrence** — two entities are linked when some `Article` `HAS_ENTITY`
//!     *both*. This is the CVE-pack default, whose builder skips `ENTITY_RELATION`
//!     edges.
//!   * **relation** — traverse explicit `Entity`→`Entity` `ENTITY_RELATION` edges
//!     (built only under `--with-entity-relations`).
//!
//! [`EntityGraphMode::Auto`] picks `relation` when the pack has any
//! `ENTITY_RELATION` edge, else `co-occurrence`. Results are bounded and
//! deterministically ordered (depth ASC, then name ASC). See
//! `docs/entity-graph.md`.

use std::collections::HashMap;

use kgpacks_db::{Connection, LogicalType, Value};

use crate::errors::{QueryError, Result};
use crate::row::to_id_string;

/// Minimum neighborhood radius.
const MIN_DEPTH: i64 = 1;
/// Maximum neighborhood radius.
const MAX_DEPTH: i64 = 3;
/// Default cap on total nodes AND per-expansion fan-out (bounds hub seeds).
const DEFAULT_LIMIT: usize = 50;

/// Traversal mode. [`EntityGraphMode::Auto`] selects [`ResolvedEntityGraphMode::Relation`]
/// when `ENTITY_RELATION` edges exist, else [`ResolvedEntityGraphMode::CoOccurrence`].
///
/// Mirrors the TypeScript `EntityGraphMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EntityGraphMode {
    /// Pick `relation` when the pack has any `ENTITY_RELATION` edge, else
    /// `co-occurrence`.
    #[default]
    Auto,
    /// Force co-occurrence traversal (shared-article links).
    CoOccurrence,
    /// Force explicit `ENTITY_RELATION` traversal.
    Relation,
}

impl EntityGraphMode {
    /// The stable wire string for this mode (`"auto"` / `"co-occurrence"` /
    /// `"relation"`), matching the reference query-parameter enum.
    pub fn as_str(self) -> &'static str {
        match self {
            EntityGraphMode::Auto => "auto",
            EntityGraphMode::CoOccurrence => "co-occurrence",
            EntityGraphMode::Relation => "relation",
        }
    }

    /// Parse a wire string into an [`EntityGraphMode`]; `None` for an unknown
    /// value (the backend renders that as an `INVALID_PARAMETER` envelope).
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(EntityGraphMode::Auto),
            "co-occurrence" => Some(EntityGraphMode::CoOccurrence),
            "relation" => Some(EntityGraphMode::Relation),
            _ => None,
        }
    }
}

/// The resolved (never `auto`) mode reported in the result.
///
/// Mirrors the TypeScript `ResolvedEntityGraphMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedEntityGraphMode {
    /// Co-occurrence traversal was used.
    CoOccurrence,
    /// Explicit `ENTITY_RELATION` traversal was used.
    Relation,
}

impl ResolvedEntityGraphMode {
    /// The stable wire string for this resolved mode.
    pub fn as_str(self) -> &'static str {
        match self {
            ResolvedEntityGraphMode::CoOccurrence => "co-occurrence",
            ResolvedEntityGraphMode::Relation => "relation",
        }
    }
}

/// Options for [`entity_graph`]. Mirrors the TypeScript `EntityGraphOptions`.
#[derive(Debug, Clone)]
pub struct EntityGraphOptions {
    /// Seed entity id (`Entity` primary key). Required.
    pub entity: String,
    /// Neighborhood radius, `1..=3` (default `1`).
    pub depth: Option<i64>,
    /// Restrict neighbors (depth > 0) to this entity type.
    pub type_filter: Option<String>,
    /// Traversal mode (default [`EntityGraphMode::Auto`]).
    pub mode: EntityGraphMode,
    /// Optional cap on the total number of nodes returned (default `50`).
    pub limit: Option<usize>,
}

impl EntityGraphOptions {
    /// Build options for a seed entity with every other field defaulted.
    pub fn new(entity: impl Into<String>) -> Self {
        Self {
            entity: entity.into(),
            depth: None,
            type_filter: None,
            mode: EntityGraphMode::Auto,
            limit: None,
        }
    }
}

/// One entity node in the neighborhood. Mirrors the TypeScript `EntityGraphNode`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityGraphNode {
    /// Entity primary key.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Entity type.
    pub type_: String,
    /// Hop distance from the seed (`0` = seed).
    pub depth: i64,
    /// Number of `Article`s that `HAS_ENTITY` this entity.
    pub articles_count: i64,
}

/// One weighted edge between two entity nodes. Mirrors the TypeScript
/// `EntityGraphEdge`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityGraphEdge {
    /// Source entity id.
    pub source: String,
    /// Target entity id.
    pub target: String,
    /// Relation label (`co_occurs` in co-occurrence mode; the stored relation in
    /// relation mode). `None` only when a relation-mode edge has a null label.
    pub relation: Option<String>,
    /// Co-occurrence count (co-occurrence mode) or `1` (relation mode).
    pub weight: i64,
}

/// The entity neighborhood returned by [`entity_graph`]. Mirrors the TypeScript
/// `EntityGraphResult`.
#[derive(Debug, Clone)]
pub struct EntityGraphResult {
    /// The (canonicalized) seed entity id.
    pub seed: String,
    /// The resolved traversal mode.
    pub mode: ResolvedEntityGraphMode,
    /// The neighborhood nodes, ordered `(depth ASC, name ASC)`.
    pub nodes: Vec<EntityGraphNode>,
    /// The edges among the returned nodes.
    pub edges: Vec<EntityGraphEdge>,
    /// `nodes.len()`.
    pub total_nodes: usize,
    /// `edges.len()`.
    pub total_edges: usize,
    /// Wall-clock traversal time in milliseconds.
    pub execution_time_ms: f64,
}

/// A neighbor discovered during expansion.
#[derive(Clone)]
struct Neighbor {
    id: String,
    name: String,
    type_: String,
}

/// A found node with the shortest depth it was first reached at.
#[derive(Clone)]
struct Found {
    id: String,
    name: String,
    type_: String,
    depth: i64,
}

/// Coerce a LadybugDB aggregate/count [`Value`] to `i64`; non-integer values
/// (`NULL`, strings, …) coerce to `0`, matching the TypeScript `Number(x ?? 0)`
/// guard where a missing count must not poison the total.
fn count_to_i64(value: Option<&Value>) -> i64 {
    match value {
        Some(Value::Int64(n)) => *n,
        Some(Value::Int32(n)) => i64::from(*n),
        Some(Value::Int16(n)) => i64::from(*n),
        Some(Value::Int8(n)) => i64::from(*n),
        Some(Value::UInt64(n)) => i64::try_from(*n).unwrap_or(i64::MAX),
        Some(Value::UInt32(n)) => i64::from(*n),
        Some(Value::UInt16(n)) => i64::from(*n),
        Some(Value::UInt8(n)) => i64::from(*n),
        Some(Value::Int128(n)) => i64::try_from(*n).unwrap_or(i64::MAX),
        _ => 0,
    }
}

/// Coerce a string-ish column [`Value`] to `String`, falling back to `default`
/// for `NULL`/absent (mirrors the reference `r.name ?? toIdString(r.id)`).
fn string_or(value: Option<&Value>, default: &str) -> String {
    match value {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null(_)) | None => default.to_string(),
        Some(other) => other.to_string(),
    }
}

/// Bind a `LIST<STRING>` parameter for an `IN $ids` predicate.
fn id_list(ids: &[String]) -> Value {
    Value::List(
        LogicalType::String,
        ids.iter().map(|id| Value::String(id.clone())).collect(),
    )
}

/// True when the pack has any `ENTITY_RELATION` edge. A missing table (an older
/// pack) surfaces as an error from the driver, which is mapped to `false`
/// (co-occurrence only) — mirroring the reference `try { … } catch { false }`.
fn has_relation_edges(conn: &Connection<'_>) -> bool {
    match conn.run("MATCH ()-[r:ENTITY_RELATION]->() RETURN count(r) AS c") {
        Ok(rows) => count_to_i64(rows.first().and_then(|r| r.get("c"))) > 0,
        Err(_) => false,
    }
}

/// Neighbors of one entity in the current mode, optionally type-restricted and
/// fan-out-capped (ordered by name for determinism) so a high-degree hub entity
/// cannot blow up the traversal.
fn neighbors_of(
    conn: &Connection<'_>,
    id: &str,
    mode: ResolvedEntityGraphMode,
    type_filter: Option<&str>,
    cap: usize,
) -> Result<Vec<Neighbor>> {
    let type_clause = if type_filter.is_some() {
        " AND e2.type = $type"
    } else {
        ""
    };
    let tail = " RETURN DISTINCT e2.entity_id AS id, e2.name AS name, e2.type AS type \
                 ORDER BY name ASC, id ASC LIMIT $cap";
    let cypher = match mode {
        ResolvedEntityGraphMode::Relation => format!(
            "MATCH (e1:Entity {{entity_id: $id}})-[:ENTITY_RELATION]-(e2:Entity) \
             WHERE e2.entity_id <> $id{type_clause}{tail}"
        ),
        ResolvedEntityGraphMode::CoOccurrence => format!(
            "MATCH (e1:Entity {{entity_id: $id}})<-[:HAS_ENTITY]-(:Article)-[:HAS_ENTITY]->(e2:Entity) \
             WHERE e2.entity_id <> $id{type_clause}{tail}"
        ),
    };
    let mut params = vec![
        ("id", Value::String(id.to_string())),
        ("cap", Value::Int64(i64::try_from(cap).unwrap_or(i64::MAX))),
    ];
    if let Some(type_filter) = type_filter {
        params.push(("type", Value::String(type_filter.to_string())));
    }
    let rows = conn.run_params(&cypher, params)?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let raw_id = row
                .get("id")
                .cloned()
                .unwrap_or(Value::Null(LogicalType::String));
            let id = to_id_string(&raw_id);
            let name = string_or(row.get("name"), &id);
            let type_ = string_or(row.get("type"), "");
            Neighbor { id, name, type_ }
        })
        .collect())
}

/// `Article` counts (`HAS_ENTITY` in-degree) for a set of entity ids.
fn article_counts(conn: &Connection<'_>, ids: &[String]) -> Result<HashMap<String, i64>> {
    let mut counts = HashMap::new();
    if ids.is_empty() {
        return Ok(counts);
    }
    let rows = conn.run_params(
        "MATCH (a:Article)-[:HAS_ENTITY]->(e:Entity) \
         WHERE e.entity_id IN $ids \
         RETURN e.entity_id AS id, count(a) AS c",
        vec![("ids", id_list(ids))],
    )?;
    for row in rows {
        let raw_id = row
            .get("id")
            .cloned()
            .unwrap_or(Value::Null(LogicalType::String));
        counts.insert(to_id_string(&raw_id), count_to_i64(row.get("c")));
    }
    Ok(counts)
}

/// Edges among the selected nodes for the active mode.
fn edges_among(
    conn: &Connection<'_>,
    ids: &[String],
    mode: ResolvedEntityGraphMode,
) -> Result<Vec<EntityGraphEdge>> {
    if ids.len() <= 1 {
        return Ok(Vec::new());
    }
    match mode {
        ResolvedEntityGraphMode::Relation => {
            let rows = conn.run_params(
                "MATCH (e1:Entity)-[r:ENTITY_RELATION]->(e2:Entity) \
                 WHERE e1.entity_id IN $ids AND e2.entity_id IN $ids \
                 RETURN DISTINCT e1.entity_id AS source, e2.entity_id AS target, r.relation AS relation",
                vec![("ids", id_list(ids))],
            )?;
            Ok(rows
                .into_iter()
                .map(|row| {
                    let source = to_id_string(
                        &row.get("source")
                            .cloned()
                            .unwrap_or(Value::Null(LogicalType::String)),
                    );
                    let target = to_id_string(
                        &row.get("target")
                            .cloned()
                            .unwrap_or(Value::Null(LogicalType::String)),
                    );
                    let relation = match row.get("relation") {
                        Some(Value::String(s)) => Some(s.clone()),
                        Some(Value::Null(_)) | None => None,
                        Some(other) => Some(other.to_string()),
                    };
                    EntityGraphEdge {
                        source,
                        target,
                        relation,
                        weight: 1,
                    }
                })
                .collect())
        }
        ResolvedEntityGraphMode::CoOccurrence => {
            // One undirected edge per pair, weight = shared article count.
            let rows = conn.run_params(
                "MATCH (e1:Entity)<-[:HAS_ENTITY]-(a:Article)-[:HAS_ENTITY]->(e2:Entity) \
                 WHERE e1.entity_id IN $ids AND e2.entity_id IN $ids AND e1.entity_id < e2.entity_id \
                 RETURN e1.entity_id AS source, e2.entity_id AS target, count(DISTINCT a) AS weight",
                vec![("ids", id_list(ids))],
            )?;
            Ok(rows
                .into_iter()
                .map(|row| {
                    let source = to_id_string(
                        &row.get("source")
                            .cloned()
                            .unwrap_or(Value::Null(LogicalType::String)),
                    );
                    let target = to_id_string(
                        &row.get("target")
                            .cloned()
                            .unwrap_or(Value::Null(LogicalType::String)),
                    );
                    EntityGraphEdge {
                        source,
                        target,
                        relation: Some("co_occurs".to_string()),
                        weight: count_to_i64(row.get("weight")),
                    }
                })
                .collect())
        }
    }
}

/// Builds the bounded entity neighborhood around `options.entity`.
///
/// Fails with [`QueryError::InvalidArgument`] for an out-of-range depth (must be
/// `1..=3`) and with [`QueryError::EntityNotFound`] for an unknown seed. Nodes are
/// ordered `(depth ASC, name ASC)` for a deterministic, transport-stable result.
///
/// Mirrors the TypeScript `entityGraph`.
pub fn entity_graph(
    conn: &Connection<'_>,
    options: &EntityGraphOptions,
) -> Result<EntityGraphResult> {
    let start = std::time::Instant::now();

    let depth = options.depth.unwrap_or(1);
    if !(MIN_DEPTH..=MAX_DEPTH).contains(&depth) {
        return Err(QueryError::InvalidArgument(format!(
            "depth must be an integer between {MIN_DEPTH} and {MAX_DEPTH}, got {depth}"
        )));
    }

    let seed_rows = conn.run_params(
        "MATCH (e:Entity {entity_id: $id}) RETURN e.entity_id AS id, e.name AS name, e.type AS type",
        vec![("id", Value::String(options.entity.clone()))],
    )?;
    let Some(seed_row) = seed_rows.first() else {
        return Err(QueryError::EntityNotFound(options.entity.clone()));
    };
    let seed_id = to_id_string(
        &seed_row
            .get("id")
            .cloned()
            .unwrap_or(Value::Null(LogicalType::String)),
    );
    let seed_name = string_or(seed_row.get("name"), &seed_id);
    let seed_type = string_or(seed_row.get("type"), "");

    let mode = match options.mode {
        EntityGraphMode::Auto => {
            if has_relation_edges(conn) {
                ResolvedEntityGraphMode::Relation
            } else {
                ResolvedEntityGraphMode::CoOccurrence
            }
        }
        EntityGraphMode::CoOccurrence => ResolvedEntityGraphMode::CoOccurrence,
        EntityGraphMode::Relation => ResolvedEntityGraphMode::Relation,
    };

    let limit = match options.limit {
        Some(limit) if limit > 0 => limit,
        _ => DEFAULT_LIMIT,
    };

    // Breadth-first expansion, recording the FIRST (shortest) depth an entity is
    // reached at. The seed's own type is never subject to the type filter. Each
    // expansion is fan-out-capped at `limit` (ordered by name) to bound hub seeds.
    let mut found: HashMap<String, Found> = HashMap::new();
    found.insert(
        seed_id.clone(),
        Found {
            id: seed_id.clone(),
            name: seed_name,
            type_: seed_type,
            depth: 0,
        },
    );
    let mut frontier = vec![seed_id.clone()];
    let mut d = 1;
    while d <= depth && !frontier.is_empty() {
        let mut next = Vec::new();
        for node_id in &frontier {
            let neighbors =
                neighbors_of(conn, node_id, mode, options.type_filter.as_deref(), limit)?;
            for neighbor in neighbors {
                if found.contains_key(&neighbor.id) {
                    continue;
                }
                found.insert(
                    neighbor.id.clone(),
                    Found {
                        id: neighbor.id.clone(),
                        name: neighbor.name,
                        type_: neighbor.type_,
                        depth: d,
                    },
                );
                next.push(neighbor.id);
            }
        }
        frontier = next;
        d += 1;
    }

    // Deterministic order: depth ASC, then name ASC, then id ASC as a stable
    // tiebreaker (two distinct entities can share a name — the `HashMap` iteration
    // order is unspecified, so without the id tiebreak a name collision at the
    // `limit` truncation boundary could reorder non-deterministically).
    let mut ordered: Vec<Found> = found.into_values().collect();
    ordered.sort_by(|a, b| {
        a.depth
            .cmp(&b.depth)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.id.cmp(&b.id))
    });
    if ordered.len() > limit {
        ordered.truncate(limit);
    }

    let ids: Vec<String> = ordered.iter().map(|n| n.id.clone()).collect();
    let counts = article_counts(conn, &ids)?;
    let nodes: Vec<EntityGraphNode> = ordered
        .into_iter()
        .map(|n| EntityGraphNode {
            articles_count: counts.get(&n.id).copied().unwrap_or(0),
            id: n.id,
            name: n.name,
            type_: n.type_,
            depth: n.depth,
        })
        .collect();

    let id_set: std::collections::HashSet<&String> = ids.iter().collect();
    let edges: Vec<EntityGraphEdge> = edges_among(conn, &ids, mode)?
        .into_iter()
        .filter(|e| id_set.contains(&e.source) && id_set.contains(&e.target))
        .collect();

    Ok(EntityGraphResult {
        seed: seed_id,
        mode,
        total_nodes: nodes.len(),
        total_edges: edges.len(),
        nodes,
        edges,
        execution_time_ms: start.elapsed().as_secs_f64() * 1000.0,
    })
}
