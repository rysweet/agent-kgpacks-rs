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

use std::fmt;

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
/// bag-of-words projection: each lowercased alphanumeric token is hashed (with
/// a fixed FNV-1a hash, so output is stable across Rust versions and platforms)
/// to a dimension and a sign, contributions are accumulated, and the vector is
/// L2-normalized. Identical text always maps to an identical unit vector, and
/// texts that share tokens are *typically* more cosine-similar than texts that
/// share none — a representative lexical similarity, though exact ordering is
/// not guaranteed under hash collisions. Token-empty text (e.g. `""` or
/// punctuation) still yields a unit vector via a sentinel token, so it never
/// collapses to a zero vector or a NaN cosine; at the 768-d production width,
/// sign-cancellation collisions for ordinary text are vanishingly unlikely
/// (only `dim == 0` yields an empty vector).
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
        let mut tokens = 0usize;
        for token in text.split(|c: char| !c.is_alphanumeric()) {
            if token.is_empty() {
                continue;
            }
            add_token(&mut vector, &token.to_lowercase());
            tokens += 1;
        }
        // Token-empty input (e.g. "" or punctuation) still gets a defined unit
        // vector: the reference transformer returns a real embedding for such
        // text (only an empty *batch* is an error), and a zero vector would make
        // a caller's cosine similarity NaN.
        if tokens == 0 {
            add_token(&mut vector, SENTINEL_TOKEN);
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

/// Sentinel token hashed for text with no alphanumeric tokens, so the resulting
/// vector is still unit-norm. Contains a NUL byte so it cannot collide with any
/// real lowercased token.
const SENTINEL_TOKEN: &str = "\u{0}empty";

/// FNV-1a 64-bit hash — a fixed, fully-specified hash so the deterministic
/// embedding is stable across Rust versions and platforms (unlike
/// `DefaultHasher`, whose output is unspecified).
fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Add one token's signed contribution to `vector` (length must be > 0): the
/// hash low bits pick the dimension, the top bit picks a stable sign.
fn add_token(vector: &mut [f32], token: &str) {
    let hash = fnv1a_64(token.as_bytes());
    let bucket = (hash % vector.len() as u64) as usize;
    let sign = if (hash >> 63) & 1 == 0 { 1.0 } else { -1.0 };
    vector[bucket] += sign;
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
