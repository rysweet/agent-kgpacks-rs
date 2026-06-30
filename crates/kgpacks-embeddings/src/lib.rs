//! `kgpacks-embeddings` — text chunking and embedding generation.
//!
//! Rust port of `@kgpacks/embeddings` (reference `bootstrap/src/embeddings`).
//! Two responsibilities:
//!
//! * [`chunker`] — overlapping, sentence-aware text chunking
//!   (`chunker.py`), used by the ingestion pipeline before embedding.
//! * [`Embedder`] / [`EmbeddingModel`] — embedding generation
//!   (`generator.py`). The reference runs a `sentence-transformers` BGE model
//!   (768-d, retrieval-optimized). Running a transformer in CI is neither
//!   hermetic nor fast, so this port ships a **deterministic** embedder that
//!   satisfies the same *retrieval contract* — fixed dimension, deterministic
//!   output, unit-norm vectors, and "texts that share words are more similar
//!   than texts that don't" — via a hashed bag-of-words projection. A real
//!   transformer backend (e.g. `candle`/`ort`) can implement [`EmbeddingModel`]
//!   later without touching the pipeline.

pub mod chunker;

pub use chunker::{
    chunk_sections, chunk_sections_with, chunk_text, chunk_text_with, Chunk, DEFAULT_CHUNK_SIZE,
    DEFAULT_OVERLAP,
};

use std::collections::hash_map::DefaultHasher;
use std::fmt;
use std::hash::{Hash, Hasher};

/// Default embedding dimensionality (matches the reference BGE model's 768).
pub const DEFAULT_DIM: usize = 768;

/// Model name reported by the deterministic embedder.
pub const DETERMINISTIC_MODEL: &str = "deterministic-hash-v1";

/// Reference BGE model name (used to gate the query prefix, parity with `generator.py`).
pub const BGE_MODEL: &str = "BAAI/bge-base-en-v1.5";

/// Retrieval query prefix required by BGE models (parity with `BGE_QUERY_PREFIX`).
pub const BGE_QUERY_PREFIX: &str = "Represent this sentence for searching relevant passages: ";

/// Error returned by batch embedding generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbeddingError {
    /// The input text/query list was empty (parity with the reference
    /// `ValueError("texts list cannot be empty")`).
    EmptyInput,
}

impl fmt::Display for EmbeddingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EmbeddingError::EmptyInput => write!(f, "input text list cannot be empty"),
        }
    }
}

impl std::error::Error for EmbeddingError {}

/// An embedding model: maps text to fixed-dimension vectors.
///
/// Object-safe, so the ingestion pipeline can hold a `&dyn EmbeddingModel` and
/// swap in a real transformer backend later. [`generate`](EmbeddingModel::generate)
/// has a default implementation in terms of [`embed`](EmbeddingModel::embed).
pub trait EmbeddingModel {
    /// Embedding dimensionality (every vector has exactly this length).
    fn dim(&self) -> usize;

    /// Embed a single document, returning a `dim()`-length vector.
    fn embed(&self, text: &str) -> Vec<f32>;

    /// Embed a batch of documents (indexing path — no query prefix).
    ///
    /// Returns [`EmbeddingError::EmptyInput`] for an empty batch, mirroring the
    /// reference's empty-list guard.
    fn generate(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        if texts.is_empty() {
            return Err(EmbeddingError::EmptyInput);
        }
        Ok(texts.iter().map(|t| self.embed(t)).collect())
    }
}

/// Deterministic, hermetic embedding generator.
///
/// Produces fixed-dimension, unit-norm vectors via a signed hashed
/// bag-of-words projection: each lowercased alphanumeric token is hashed to a
/// dimension and a sign, contributions are accumulated, and the vector is
/// L2-normalized. This makes the retrieval contract hold — identical text maps
/// to an identical vector, and texts sharing tokens have higher cosine
/// similarity than texts that share none — without any model download or
/// non-determinism.
#[derive(Debug, Clone)]
pub struct Embedder {
    dim: usize,
    model_name: String,
}

impl Embedder {
    /// Create an embedder producing vectors of `dim` floats (deterministic model).
    pub fn new(dim: usize) -> Self {
        Self {
            dim,
            model_name: DETERMINISTIC_MODEL.to_string(),
        }
    }

    /// Create an embedder mirroring the reference BGE model: 768 dimensions and
    /// the BGE model name (which enables the query prefix in
    /// [`generate_query`](Embedder::generate_query)).
    pub fn bge() -> Self {
        Self {
            dim: DEFAULT_DIM,
            model_name: BGE_MODEL.to_string(),
        }
    }

    /// Embedding dimensionality.
    pub fn dim(&self) -> usize {
        self.dim
    }

    /// The configured model name.
    pub fn model_name(&self) -> &str {
        &self.model_name
    }

    /// Deterministic embedding for a single document.
    pub fn embed(&self, text: &str) -> Vec<f32> {
        let mut vector = vec![0f32; self.dim];
        if self.dim == 0 {
            return vector;
        }
        for token in text.split(|c: char| !c.is_alphanumeric()) {
            if token.is_empty() {
                continue;
            }
            let lowered = token.to_lowercase();
            let mut hasher = DefaultHasher::new();
            lowered.hash(&mut hasher);
            let hash = hasher.finish();
            let bucket = (hash % self.dim as u64) as usize;
            // A second hash bit gives each token a stable sign, spreading mass
            // across the space so unrelated tokens are less likely to reinforce.
            let sign = if (hash >> 32) & 1 == 0 { 1.0 } else { -1.0 };
            vector[bucket] += sign;
        }
        l2_normalize(&mut vector);
        vector
    }

    /// Embed a batch of documents (indexing path). See
    /// [`EmbeddingModel::generate`].
    pub fn generate(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        EmbeddingModel::generate(self, texts)
    }

    /// Embed a batch of search queries. For BGE-family models the BGE retrieval
    /// prefix is prepended to each query (parity with `generate_query`); for any
    /// other model the queries are embedded unchanged.
    pub fn generate_query(&self, queries: &[&str]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        if queries.is_empty() {
            return Err(EmbeddingError::EmptyInput);
        }
        let is_bge = self.model_name.to_lowercase().contains("bge");
        Ok(queries
            .iter()
            .map(|q| {
                if is_bge {
                    self.embed(&format!("{BGE_QUERY_PREFIX}{q}"))
                } else {
                    self.embed(q)
                }
            })
            .collect())
    }
}

impl EmbeddingModel for Embedder {
    fn dim(&self) -> usize {
        Embedder::dim(self)
    }

    fn embed(&self, text: &str) -> Vec<f32> {
        Embedder::embed(self, text)
    }
}

/// L2-normalize `vector` in place; a zero vector is left unchanged.
fn l2_normalize(vector: &mut [f32]) {
    let norm = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in vector.iter_mut() {
            *x /= norm;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embeds_to_fixed_dimension() {
        let e = Embedder::new(8);
        assert_eq!(e.dim(), 8);
        assert_eq!(e.embed("hello").len(), 8);
    }

    #[test]
    fn bge_has_768_dims_and_model_name() {
        let e = Embedder::bge();
        assert_eq!(e.dim(), DEFAULT_DIM);
        assert_eq!(e.model_name(), BGE_MODEL);
    }
}
