//! `kgpacks-query` — hybrid retrieval.
//!
//! Rust port of `@kgpacks/query`'s `hybrid.ts`. Combines three signals into a
//! single weighted score per node, mirroring the reference `hybrid_retrieve`:
//!
//!   1. vector  — cosine similarity         (`+= vector_weight  * similarity`)
//!   2. graph   — `LINKS_TO` proximity      (`+= graph_weight   * 0.5` per neighbor)
//!   3. keyword — title `CONTAINS` match    (`+= keyword_weight * 0.7` per match)
//!
//! Scores accumulate in **insertion order** so the first scored nodes seed the
//! graph traversal exactly as the reference does (the first 3 vector hits become
//! the 3 graph seeds; the first 3 significant query terms become the keywords).

use std::collections::{HashMap, HashSet};

use kgpacks_db::{Connection, LogicalType, Value};

use crate::constants::{
    GRAPH_MATCH, KEYWORD_MATCH, MAX_GRAPH_SEEDS, MAX_KEYWORDS, MIN_KEYWORD_LENGTH,
};
use crate::errors::Result;
use crate::row::{coerce_content, to_id_string};
use crate::types::{Embedder, HybridWeights, RetrieverResult};
use crate::vector::{run_vector_search, VectorConfig};

/// First-seen content + raw key for a node, kept so the final ranking can render
/// content and the graph seeds can re-bind their primary key.
struct NodeMeta {
    raw_id: Value,
    content: String,
}

/// Insertion-ordered score accumulator. Mirrors the TypeScript pair of `Map`s
/// (`scored` + `meta`): order is preserved via `order`, scores accumulate in
/// `scored`, and `meta` keeps the **first-seen** raw id + content per node.
#[derive(Default)]
struct Accumulator {
    order: Vec<String>,
    scored: HashMap<String, f64>,
    meta: HashMap<String, NodeMeta>,
}

impl Accumulator {
    fn add(&mut self, id: String, raw_id: Value, content: String, delta: f64) {
        if !self.scored.contains_key(&id) {
            self.order.push(id.clone());
            self.meta.insert(id.clone(), NodeMeta { raw_id, content });
        }
        *self.scored.entry(id).or_insert(0.0) += delta;
    }
}

/// Hybrid retrieval: blends vector, graph-proximity, and keyword signals into a
/// single ranking. Mirrors the TypeScript `hybridRetrieve`.
pub fn hybrid_retrieve<E: Embedder + ?Sized>(
    conn: &Connection<'_>,
    embedder: &E,
    query: &str,
    k: usize,
    weights: &HybridWeights,
    config: &VectorConfig,
    stop_words: &HashSet<String>,
) -> Result<Vec<RetrieverResult>> {
    let mut acc = Accumulator::default();

    // Signal 1: vector similarity.
    for hit in run_vector_search(conn, embedder, query, k, config)? {
        acc.add(hit.id, hit.raw_id, hit.content, weights.vector * hit.score);
    }

    // Signal 2: graph proximity from the first few scored nodes' LINKS_TO edges.
    let seed_ids: Vec<String> = acc.order.iter().take(MAX_GRAPH_SEEDS).cloned().collect();
    let graph_cypher = format!(
        "MATCH (seed:{nt} {{id: $id}})-[:LINKS_TO]->(neighbor:{nt}) \
         RETURN neighbor.id AS id, neighbor.content AS content \
         LIMIT $limit",
        nt = config.node_table
    );
    for seed_id in seed_ids {
        let Some(seed_raw) = acc.meta.get(&seed_id).map(|m| m.raw_id.clone()) else {
            continue;
        };
        let neighbors = conn.run_params(
            &graph_cypher,
            vec![("id", seed_raw), ("limit", Value::Int64(k as i64))],
        )?;
        for row in neighbors {
            let raw_id = row
                .get("id")
                .cloned()
                .unwrap_or(Value::Null(LogicalType::Int64));
            let content = row.get("content").map(coerce_content).unwrap_or_default();
            acc.add(
                to_id_string(&raw_id),
                raw_id,
                content,
                weights.graph * GRAPH_MATCH,
            );
        }
    }

    // Signal 3: keyword title matches (first 3 significant query terms).
    let keywords: Vec<&str> = query
        .split_whitespace()
        .filter(|word| {
            word.chars().count() > MIN_KEYWORD_LENGTH && !stop_words.contains(&word.to_lowercase())
        })
        .take(MAX_KEYWORDS)
        .collect();
    let keyword_cypher = format!(
        "MATCH (s:{nt}) WHERE lower(s.title) CONTAINS lower($kw) \
         RETURN s.id AS id, s.content AS content \
         LIMIT $limit",
        nt = config.node_table
    );
    for keyword in keywords {
        let hits = conn.run_params(
            &keyword_cypher,
            vec![
                ("kw", Value::String(keyword.to_string())),
                ("limit", Value::Int64(k as i64)),
            ],
        )?;
        for row in hits {
            let raw_id = row
                .get("id")
                .cloned()
                .unwrap_or(Value::Null(LogicalType::Int64));
            let content = row.get("content").map(coerce_content).unwrap_or_default();
            acc.add(
                to_id_string(&raw_id),
                raw_id,
                content,
                weights.keyword * KEYWORD_MATCH,
            );
        }
    }

    // Rank by the weighted sum (descending), stable on insertion order for ties,
    // then truncate to k — mirroring the reference's stable sort over the
    // insertion-ordered score map.
    let mut entries: Vec<(String, f64)> = acc
        .order
        .iter()
        .map(|id| (id.clone(), acc.scored[id]))
        .collect();
    entries.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    entries.truncate(k);

    Ok(entries
        .into_iter()
        .map(|(id, score)| {
            let content = acc
                .meta
                .get(&id)
                .map(|m| m.content.clone())
                .unwrap_or_default();
            RetrieverResult { id, score, content }
        })
        .collect())
}
