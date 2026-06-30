//! `kgpacks-query` — vector retrieval.
//!
//! Rust port of `@kgpacks/query`'s `vector.ts`. Embeds the query, runs a cosine
//! vector search over a LadybugDB pack via `CALL QUERY_VECTOR_INDEX`, and returns
//! nodes ranked by similarity. Ported from the reference `semantic_search` vector
//! path: `score = clamp(1 - distance, 0, 1)`, nearest first.

use kgpacks_db::{Connection, LogicalType, Value};

use crate::constants::DEFAULT_SIMILARITY;
use crate::errors::Result;
use crate::row::{clamp01, coerce_content, to_id_string};
use crate::types::{Embedder, RetrieverResult};

/// Schema coordinates for the vector index to search. Mirrors the TypeScript
/// `VectorConfig`.
#[derive(Debug, Clone)]
pub struct VectorConfig {
    /// Node table holding the embeddings (e.g. `Section`).
    pub node_table: String,
    /// Vector index name created over that table (e.g. `embedding_idx`).
    pub vector_index: String,
}

/// A vector hit enriched with its raw primary key (for graph re-binding in the
/// hybrid path). Mirrors the TypeScript `ScoredNode`.
#[derive(Debug, Clone)]
pub struct ScoredNode {
    /// Raw primary key [`Value`] as returned by the driver (re-bound by the
    /// hybrid graph traversal).
    pub raw_id: Value,
    /// Stable string form of [`ScoredNode::raw_id`].
    pub id: String,
    /// Cosine similarity `1 - distance`, clamped to `[0, 1]`.
    pub score: f64,
    /// The node's section content.
    pub content: String,
}

/// Coerce a numeric [`Value`] to `f64`; returns `None` for non-numeric values so
/// the caller can fall back to [`DEFAULT_SIMILARITY`] (mirroring the TypeScript
/// `Number(row.distance)` + `Number.isFinite` guard, where a missing/non-numeric
/// distance must NOT poison the score with `NaN`).
fn value_to_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Double(d) => Some(*d),
        Value::Float(f) => Some(f64::from(*f)),
        Value::Int64(n) => Some(*n as f64),
        Value::Int32(n) => Some(f64::from(*n)),
        Value::Int16(n) => Some(f64::from(*n)),
        Value::Int8(n) => Some(f64::from(*n)),
        Value::UInt64(n) => Some(*n as f64),
        Value::UInt32(n) => Some(f64::from(*n)),
        _ => None,
    }
}

/// Build the FLOAT-list bound parameter for the query embedding.
///
/// `QUERY_VECTOR_INDEX` binds the query vector as a list, not a fixed-size array;
/// the engine casts it to the index's `FLOAT[dim]` element type. We bind a
/// `LIST<FLOAT>` so the float column type matches the index without an explicit
/// `ARRAY` length.
fn embedding_param(embedding: &[f32]) -> Value {
    Value::List(
        LogicalType::Float,
        embedding.iter().map(|&x| Value::Float(x)).collect(),
    )
}

/// Runs the cosine vector search and returns hits enriched with `raw_id`.
///
/// `node_table`/`vector_index` are trusted configuration (developer-supplied,
/// never end-user input) and are interpolated as the procedure's string-literal
/// arguments, mirroring `CALL QUERY_VECTOR_INDEX('Section', 'embedding_idx', …)`.
/// The embedding vector and `k` are **bound** parameters.
pub fn run_vector_search<E: Embedder + ?Sized>(
    conn: &Connection<'_>,
    embedder: &E,
    query: &str,
    k: usize,
    config: &VectorConfig,
) -> Result<Vec<ScoredNode>> {
    let embeddings = embedder.generate_query(&[query])?;
    let Some(embedding) = embeddings.into_iter().next() else {
        return Ok(Vec::new());
    };

    let cypher = format!(
        "CALL QUERY_VECTOR_INDEX('{}', '{}', $emb, $k) \
         RETURN node.id AS id, node.content AS content, distance AS distance \
         ORDER BY distance",
        config.node_table, config.vector_index
    );
    let rows = conn.run_params(
        &cypher,
        vec![
            ("emb", embedding_param(&embedding)),
            ("k", Value::Int64(k as i64)),
        ],
    )?;

    Ok(rows
        .into_iter()
        .map(|row| {
            let score = match row.get("distance").and_then(value_to_f64) {
                Some(distance) if distance.is_finite() => clamp01(1.0 - distance),
                _ => DEFAULT_SIMILARITY,
            };
            let raw_id = row
                .get("id")
                .cloned()
                .unwrap_or(Value::Null(LogicalType::Int64));
            let content = row.get("content").map(coerce_content).unwrap_or_default();
            ScoredNode {
                id: to_id_string(&raw_id),
                raw_id,
                score,
                content,
            }
        })
        .collect())
}

/// Vector retrieval: top-k nodes ranked by cosine similarity (highest first).
///
/// Mirrors the TypeScript `vectorRetrieve`.
pub fn vector_retrieve<E: Embedder + ?Sized>(
    conn: &Connection<'_>,
    embedder: &E,
    query: &str,
    k: usize,
    config: &VectorConfig,
) -> Result<Vec<RetrieverResult>> {
    Ok(run_vector_search(conn, embedder, query, k, config)?
        .into_iter()
        .map(|hit| RetrieverResult {
            id: hit.id,
            score: hit.score,
            content: hit.content,
        })
        .collect())
}
