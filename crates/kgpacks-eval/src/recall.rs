//! Deterministic retrieval-recall (embedding hit@k).
//!
//! The LLM-free fallback metric for full-pack CVE eval validation. When no pinned
//! judge/synthesis transport is available (the offline/CI case), the eval report
//! is a deterministic sanity check on the **retrieval half** of the with-pack
//! arm: each question is embedded and matched (cosine) against a corpus of the
//! pack's records, and recall@k is the fraction of questions whose target record
//! appears in the top-k.
//!
//! It is fully deterministic — the [`Embedder`](kgpacks_embeddings::Embedder)
//! produces stable unit-norm vectors — so the committed numbers are reproducible
//! and CI-guardable.

use std::collections::BTreeMap;

use kgpacks_embeddings::Embedder;

/// A corpus record the queries are matched against.
#[derive(Debug, Clone)]
pub struct RecallDoc {
    /// Stable record id (the retrieval target key).
    pub id: String,
    /// Record text embedded on the indexing path.
    pub text: String,
}

/// A query and the id of the corpus record it should retrieve.
#[derive(Debug, Clone)]
pub struct RecallQuery {
    /// The question text embedded on the query path.
    pub question: String,
    /// The [`RecallDoc::id`] that counts as a hit for this query.
    pub target_id: String,
}

/// Compute recall@k for each `k` in `ks` over `queries` against `corpus`.
///
/// Each document is embedded on the indexing path and each query on the query
/// path (the BGE retrieval prefix is applied for BGE-family embedders). Documents
/// are ranked by descending cosine similarity to the query; a query is a hit at
/// `k` when its `target_id` appears within the top-`k`. Returns `k → recall` in
/// ascending `k` order. Empty `queries` yields `0.0` for every `k` (never `NaN`);
/// a `target_id` absent from `corpus` is always a miss.
pub fn recall_at_k(
    embedder: &Embedder,
    queries: &[RecallQuery],
    corpus: &[RecallDoc],
    ks: &[usize],
) -> BTreeMap<usize, f64> {
    let doc_vectors: Vec<(usize, Vec<f32>)> = corpus
        .iter()
        .enumerate()
        .map(|(idx, doc)| (idx, embedder.embed(&doc.text)))
        .collect();

    let mut hits_at: BTreeMap<usize, usize> = ks.iter().map(|&k| (k, 0usize)).collect();

    for query in queries {
        let target_idx = corpus.iter().position(|doc| doc.id == query.target_id);
        let query_vec = embed_query(embedder, &query.question);

        // Rank docs by descending cosine similarity (deterministic tie-break by
        // original index, so equal-scoring docs keep a stable order).
        let mut scored: Vec<(usize, f32)> = doc_vectors
            .iter()
            .map(|(idx, vec)| (*idx, cosine(&query_vec, vec)))
            .collect();
        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.0.cmp(&b.0))
        });

        let rank = target_idx.and_then(|t| scored.iter().position(|(idx, _)| *idx == t));
        if let Some(rank) = rank {
            for (&k, hits) in hits_at.iter_mut() {
                if rank < k {
                    *hits += 1;
                }
            }
        }
    }

    let total = queries.len();
    hits_at
        .into_iter()
        .map(|(k, hits)| {
            let recall = if total == 0 {
                0.0
            } else {
                hits as f64 / total as f64
            };
            (k, recall)
        })
        .collect()
}

/// Embed a query, applying the BGE retrieval prefix for BGE-family embedders.
///
/// `generate_query` returns exactly one vector for a single non-empty query and
/// only errors on an empty batch (which cannot happen here), so the trailing
/// `embed` arm is unreachable defensive code — not a silent degradation path.
fn embed_query(embedder: &Embedder, question: &str) -> Vec<f32> {
    embedder
        .generate_query(&[question])
        .ok()
        .and_then(|mut vectors| vectors.pop())
        .unwrap_or_else(|| embedder.embed(question))
}

/// Cosine similarity of two vectors. The embedder yields unit-norm vectors, so
/// this is their dot product; a zero-length input yields `0.0`.
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let len = a.len().min(b.len());
    if len == 0 {
        return 0.0;
    }
    (0..len).map(|i| a[i] * b[i]).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(id: &str, text: &str) -> RecallDoc {
        RecallDoc {
            id: id.into(),
            text: text.into(),
        }
    }

    #[test]
    fn perfect_recall_when_query_matches_its_doc_text() {
        let embedder = Embedder::bge();
        let corpus = vec![
            doc("a", "alpha bravo charlie delta"),
            doc("b", "echo foxtrot golf hotel"),
            doc("c", "india juliet kilo lima"),
        ];
        let queries = vec![
            RecallQuery {
                question: "alpha bravo charlie delta".into(),
                target_id: "a".into(),
            },
            RecallQuery {
                question: "echo foxtrot golf hotel".into(),
                target_id: "b".into(),
            },
        ];
        let recall = recall_at_k(&embedder, &queries, &corpus, &[1, 3]);
        assert_eq!(recall[&1], 1.0);
        assert_eq!(recall[&3], 1.0);
    }

    #[test]
    fn empty_queries_yield_zero_not_nan() {
        let embedder = Embedder::bge();
        let corpus = vec![doc("a", "alpha")];
        let recall = recall_at_k(&embedder, &[], &corpus, &[1, 5]);
        assert_eq!(recall[&1], 0.0);
        assert_eq!(recall[&5], 0.0);
    }

    #[test]
    fn missing_target_is_a_miss() {
        let embedder = Embedder::bge();
        let corpus = vec![doc("a", "alpha bravo")];
        let queries = vec![RecallQuery {
            question: "alpha bravo".into(),
            target_id: "does-not-exist".into(),
        }];
        let recall = recall_at_k(&embedder, &queries, &corpus, &[1]);
        assert_eq!(recall[&1], 0.0);
    }

    #[test]
    fn is_deterministic_across_runs() {
        let embedder = Embedder::bge();
        let corpus = vec![doc("a", "one two three"), doc("b", "four five six")];
        let queries = vec![RecallQuery {
            question: "one two three".into(),
            target_id: "a".into(),
        }];
        let first = recall_at_k(&embedder, &queries, &corpus, &[1, 2]);
        let second = recall_at_k(&embedder, &queries, &corpus, &[1, 2]);
        assert_eq!(first, second);
    }
}
