//! Shared OFFLINE test helpers for the `kgpacks-query` parity suite.
//!
//! These mirror the deterministic fakes used by `@kgpacks/query`'s test suite
//! (`packages/query/test/helpers.ts` + the inline `fixedEmbedder` of
//! `hybrid.test.ts`): an injected embedder with KNOWN vectors so cosine
//! similarities are exact and the only variability under test is the retrieval
//! formula itself. No network, no real model.
//!
//! `mod common;` is included by several test binaries; not every binary uses
//! every helper, so dead-code warnings are suppressed crate-locally here.
#![allow(dead_code)]

use kgpacks_db::{Connection, LogicalType, Value};
use kgpacks_query::{Embedder, Result};

/// Production embedding width (matches `kgpacks_embeddings::DEFAULT_DIM`).
pub const DIM: usize = 768;

/// A `DIM`-length one-hot unit vector with `1.0` at `index`.
pub fn one_hot(index: usize) -> Vec<f32> {
    let mut v = vec![0f32; DIM];
    v[index] = 1.0;
    v
}

/// A `DIM`-length unit vector `a*e_i + b*e_j` (with `i != j`). When
/// `a*a + b*b == 1` this is a unit vector whose cosine similarity with
/// `one_hot(i)` is exactly `a` — used to construct hits with known graded
/// similarity (e.g. `mix(0, 1, 0.6, 0.8)` has cosine `0.6` to `one_hot(0)`).
pub fn mix(i: usize, j: usize, a: f32, b: f32) -> Vec<f32> {
    let mut v = vec![0f32; DIM];
    v[i] = a;
    v[j] = b;
    v
}

/// A deterministic query embedder that returns the same fixed `vector` for every
/// query (mirrors the inline `fixedEmbedder` of the reference `hybrid.test.ts`).
pub struct FixedQueryEmbedder {
    pub vector: Vec<f32>,
}

impl Embedder for FixedQueryEmbedder {
    fn generate_query(&self, queries: &[&str]) -> Result<Vec<Vec<f32>>> {
        Ok(queries.iter().map(|_| self.vector.clone()).collect())
    }
}

/// Build a `FLOAT[DIM]` array bound parameter for an embedding column insert.
pub fn float_array(v: &[f32]) -> Value {
    Value::Array(
        LogicalType::Float,
        v.iter().map(|&x| Value::Float(x)).collect(),
    )
}

/// Create the `Section(id INT64 …)` + `LINKS_TO` schema and the cosine vector
/// index, mirroring the reference `hybrid.test.ts` fixture. Requires the
/// `vector` extension to already be loaded on `conn`.
pub fn create_int_schema(conn: &Connection<'_>) {
    conn.run(
        "CREATE NODE TABLE Section(id INT64, title STRING, content STRING, \
         embedding FLOAT[768], PRIMARY KEY(id))",
    )
    .expect("create Section table");
    conn.run("CREATE REL TABLE LINKS_TO(FROM Section TO Section)")
        .expect("create LINKS_TO table");
}

/// Insert one `Section` row with an `INT64` id and a `FLOAT[DIM]` embedding.
pub fn insert_int_section(
    conn: &Connection<'_>,
    id: i64,
    title: &str,
    content: &str,
    embedding: &[f32],
) {
    conn.run_params(
        "CREATE (:Section {id: $id, title: $title, content: $content, embedding: $emb})",
        vec![
            ("id", Value::Int64(id)),
            ("title", Value::String(title.to_string())),
            ("content", Value::String(content.to_string())),
            ("emb", float_array(embedding)),
        ],
    )
    .expect("insert Section");
}

/// Create a directed `LINKS_TO` edge between two `INT64`-keyed sections.
pub fn link_int(conn: &Connection<'_>, from: i64, to: i64) {
    conn.run_params(
        "MATCH (a:Section {id: $from}), (b:Section {id: $to}) CREATE (a)-[:LINKS_TO]->(b)",
        vec![("from", Value::Int64(from)), ("to", Value::Int64(to))],
    )
    .expect("create LINKS_TO edge");
}

/// Create the cosine `embedding_idx` vector index over `Section.embedding`.
pub fn create_vector_index(conn: &Connection<'_>) {
    conn.run(
        "CALL CREATE_VECTOR_INDEX('Section', 'embedding_idx', 'embedding', metric := 'cosine')",
    )
    .expect("create vector index");
}
