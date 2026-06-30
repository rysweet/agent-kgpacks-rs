//! Parity tests for embedding generation (`bootstrap/src/embeddings/test_generator.py`).
//!
//! The reference exercises a `sentence-transformers` BGE model; this port ships
//! a deterministic embedder (no model download), so the *numeric* thresholds of
//! the transformer test are adapted to the contract a deterministic model
//! guarantees: fixed `(N, dim)` shape, exact determinism, unit-norm vectors, an
//! empty-input error, and "shared-word texts are more similar than
//! disjoint-word texts" — plus the BGE query-prefix behavior of `generate_query`.

use kgpacks_embeddings::{Embedder, EmbeddingError, BGE_MODEL, DEFAULT_DIM};

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    dot / (na * nb)
}

fn sample_texts() -> Vec<&'static str> {
    vec![
        "Machine learning is a field of artificial intelligence",
        "Deep learning uses neural networks",
        "Python is a programming language",
    ]
}

#[test]
fn generate_has_shape_n_by_dim() {
    let gen = Embedder::bge();
    let embeddings = gen.generate(&sample_texts()).unwrap();
    assert_eq!(embeddings.len(), 3);
    assert!(embeddings.iter().all(|e| e.len() == DEFAULT_DIM));
}

#[test]
fn generate_is_deterministic() {
    let gen = Embedder::bge();
    let a = gen.generate(&["Consistent embedding test"]).unwrap();
    let b = gen.generate(&["Consistent embedding test"]).unwrap();
    assert_eq!(a, b);
}

#[test]
fn embeddings_are_unit_norm_and_nonzero() {
    let gen = Embedder::bge();
    for e in gen.generate(&sample_texts()).unwrap() {
        let norm = e.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-4, "norm was {norm}");
    }
}

#[test]
fn generate_single_and_large_batch() {
    let gen = Embedder::bge();
    assert_eq!(gen.generate(&["Single text"]).unwrap().len(), 1);

    let owned: Vec<String> = (0..100)
        .map(|i| format!("Sample text number {i}"))
        .collect();
    let texts: Vec<&str> = owned.iter().map(String::as_str).collect();
    let embeddings = gen.generate(&texts).unwrap();
    assert_eq!(embeddings.len(), 100);
    assert!(embeddings.iter().all(|e| e.len() == DEFAULT_DIM));
}

#[test]
fn empty_list_is_an_error() {
    let gen = Embedder::bge();
    assert_eq!(gen.generate(&[]), Err(EmbeddingError::EmptyInput));
    assert_eq!(gen.generate_query(&[]), Err(EmbeddingError::EmptyInput));
}

#[test]
fn similar_texts_rank_above_dissimilar_texts() {
    let gen = Embedder::bge();
    let similar = gen
        .generate(&["Machine learning", "Deep learning"])
        .unwrap();
    let dissimilar = gen
        .generate(&["Machine learning algorithms", "The quick brown fox"])
        .unwrap();

    let sim = cosine(&similar[0], &similar[1]);
    let dis = cosine(&dissimilar[0], &dissimilar[1]);

    // Shared-word pair must be meaningfully positive and rank above the
    // disjoint-word pair, which should be near-orthogonal.
    assert!(sim > 0.3, "expected sim > 0.3, got {sim}");
    assert!(sim > dis, "expected sim ({sim}) > dis ({dis})");
    assert!(dis.abs() < 0.25, "expected |dis| < 0.25, got {dis}");
}

#[test]
fn dim_and_model_name_are_reported() {
    let gen = Embedder::bge();
    assert_eq!(gen.dim(), DEFAULT_DIM);
    assert_eq!(gen.model_name(), BGE_MODEL);
}

#[test]
fn bge_query_generation_adds_the_prefix() {
    let bge = Embedder::bge();
    // For a BGE model, generate_query prepends the retrieval prefix, so the
    // query embedding differs from the bare document embedding.
    let doc = bge.generate(&["graph databases"]).unwrap();
    let query = bge.generate_query(&["graph databases"]).unwrap();
    assert_ne!(doc[0], query[0]);
}

#[test]
fn non_bge_query_generation_does_not_add_a_prefix() {
    let plain = Embedder::new(DEFAULT_DIM);
    let doc = plain.generate(&["graph databases"]).unwrap();
    let query = plain.generate_query(&["graph databases"]).unwrap();
    assert_eq!(doc[0], query[0]);
}
