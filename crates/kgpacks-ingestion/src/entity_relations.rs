//! Scalable bulk `ENTITY_RELATION` load.
//!
//! Rust port of the `ENTITY_RELATION` bulk path in `@kgpacks/ingestion`'s
//! `streaming-loader.ts` (`bulkCreateEntityRelations`). Loading `Entity`→`Entity`
//! relationship edges one statement at a time — as the naive per-row
//! `MATCH (e1), (e2) CREATE …` loop does — issues one round-trip per edge and, via
//! the comma two-pattern `MATCH`, hash-joins the *growing* `Entity` table against
//! itself: super-linear at corpus scale.
//!
//! [`bulk_create_entity_relations`] loads the same edges scalably. It prefers
//! LadybugDB's `COPY <Rel> FROM <csv>` — a single bulk import that scales to the
//! full corpus — and falls back to PK-indexed `UNWIND … MATCH … MATCH … CREATE`
//! batches when `COPY` is unavailable or rejects the file. **Both** shapes are
//! non-O(N²): neither uses a comma two-pattern `MATCH` over the node tables; the
//! fallback point-looks-up each endpoint by primary key with two separate `MATCH`
//! clauses.
//!
//! The caller is responsible for pre-filtering to rows whose *both* endpoints
//! already exist as `Entity` nodes (as the reference filters against its
//! deduped-entity set): `COPY <Rel>` errors on a dangling foreign key, so a
//! dangling row would abort the whole bulk import.

use std::path::Path;

use kgpacks_db::{Connection, LogicalType, Value};

use crate::error::Result;

/// Max rows per `UNWIND` statement in the fallback path (bounds the prepared
/// statement's parameter size). Mirrors the reference `RELATION_FALLBACK_CHUNK`.
const RELATION_FALLBACK_CHUNK: usize = 1000;

/// One `Entity`→`Entity` relationship edge to bulk-load.
///
/// `source_id`/`target_id` are `Entity` primary keys (`entity_id`); both must
/// already exist as nodes (see the module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityRelationRow {
    /// Source `Entity` primary key.
    pub source_id: String,
    /// Target `Entity` primary key.
    pub target_id: String,
    /// The (normalized) relation verb stored on the edge.
    pub relation: String,
    /// The sentence/clause the relationship was extracted from.
    pub context: String,
}

/// Bulk-create `ENTITY_RELATION` (`Entity`→`Entity`) edges scalably.
///
/// Prefers `COPY ENTITY_RELATION FROM <csv>` (a single bulk import); on any
/// failure — the engine build lacks `COPY`, or rejects the file — falls back to
/// PK-indexed `UNWIND … MATCH … MATCH … CREATE` batches. Returns the number of
/// edges created (COPY: the full row count; fallback: the sum of batch sizes).
///
/// An empty input is a no-op returning `0`. See the module docs for the caller's
/// pre-filtering obligation.
pub fn bulk_create_entity_relations(
    conn: &Connection<'_>,
    rows: &[EntityRelationRow],
) -> Result<usize> {
    if rows.is_empty() {
        return Ok(0);
    }
    match copy_entity_relations(conn, rows) {
        Ok(created) => Ok(created),
        // COPY unsupported/rejected → PK-indexed UNWIND fallback (still ~linear).
        Err(_) => create_entity_relations_batched(conn, rows),
    }
}

/// Load all edges via a single `COPY ENTITY_RELATION FROM <csv>` bulk import.
///
/// The CSV columns are, in `COPY <Rel>` order: the `FROM` primary key, the `TO`
/// primary key, then the edge properties in declaration order (`relation`,
/// `context`). The file is written to a fresh temp directory and removed before
/// returning (on both the success and error paths).
fn copy_entity_relations(conn: &Connection<'_>, rows: &[EntityRelationRow]) -> Result<usize> {
    let dir = tempfile::Builder::new().prefix("kgpacks-rels-").tempdir()?;
    let file = dir.path().join("entity_relation.csv");
    write_csv(&file, rows)?;
    // LadybugDB parses the path as a string literal; normalize separators so a
    // Windows path is accepted too (parity with the reference).
    let path = file.to_string_lossy().replace('\\', "/");
    conn.run(&format!("COPY ENTITY_RELATION FROM \"{path}\""))?;
    // `dir` (and the CSV) is removed when it drops here.
    Ok(rows.len())
}

/// Write the `ENTITY_RELATION` rows as RFC-4180 CSV (every field quoted, embedded
/// quotes doubled) so relation/context text with commas, quotes or newlines round
/// trips through `COPY`.
fn write_csv(path: &Path, rows: &[EntityRelationRow]) -> Result<()> {
    let mut csv = String::new();
    for row in rows {
        csv.push_str(&csv_field(&row.source_id));
        csv.push(',');
        csv.push_str(&csv_field(&row.target_id));
        csv.push(',');
        csv.push_str(&csv_field(&row.relation));
        csv.push(',');
        csv.push_str(&csv_field(&row.context));
        csv.push('\n');
    }
    std::fs::write(path, csv)?;
    Ok(())
}

/// Quote a CSV field and double any embedded quotes (RFC-4180).
fn csv_field(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

/// Load edges in PK-indexed `UNWIND … MATCH … MATCH … CREATE` batches (no `COPY`).
///
/// The [`bulk_create_entity_relations`] fallback, also exposed directly for
/// callers who know `COPY` is unavailable and for isolated testing. Each batch
/// binds a `LIST<STRUCT>` of `{s, t, rel, ctx}` rows and creates one edge per row,
/// point-looking-up each endpoint by primary key with **two separate** `MATCH`
/// clauses — never a comma two-pattern `MATCH` — so the load stays ~linear. A row
/// whose endpoint is missing is silently skipped by `MATCH` (parity with the naive
/// loop it replaces), so this path is robust to a dangling endpoint that `COPY`
/// would reject. The returned count is the number of rows *submitted* (summed
/// batch sizes), which may exceed the edges actually created when rows are skipped.
pub fn create_entity_relations_batched(
    conn: &Connection<'_>,
    rows: &[EntityRelationRow],
) -> Result<usize> {
    let mut created = 0;
    for chunk in rows.chunks(RELATION_FALLBACK_CHUNK) {
        conn.run_params(
            "UNWIND $rows AS r \
             MATCH (a:Entity {entity_id: r.s}) \
             MATCH (b:Entity {entity_id: r.t}) \
             CREATE (a)-[:ENTITY_RELATION {relation: r.rel, context: r.ctx}]->(b)",
            vec![("rows", rows_param(chunk))],
        )?;
        created += chunk.len();
    }
    Ok(created)
}

/// Build the `LIST<STRUCT<s, t, rel, ctx: STRING>>` bound parameter for an
/// `UNWIND` batch.
fn rows_param(rows: &[EntityRelationRow]) -> Value {
    let field_type = LogicalType::Struct {
        fields: vec![
            ("s".to_string(), LogicalType::String),
            ("t".to_string(), LogicalType::String),
            ("rel".to_string(), LogicalType::String),
            ("ctx".to_string(), LogicalType::String),
        ],
    };
    let structs = rows
        .iter()
        .map(|row| {
            Value::Struct(vec![
                ("s".to_string(), Value::String(row.source_id.clone())),
                ("t".to_string(), Value::String(row.target_id.clone())),
                ("rel".to_string(), Value::String(row.relation.clone())),
                ("ctx".to_string(), Value::String(row.context.clone())),
            ])
        })
        .collect();
    Value::List(field_type, structs)
}
