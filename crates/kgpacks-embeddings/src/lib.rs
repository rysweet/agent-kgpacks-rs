//! `kgpacks-embeddings` — text embeddings.
//!
//! Rust port of `@kgpacks/embeddings`. Real transformer inference
//! (HuggingFace-parity) lands in M3; the M1 scaffold returns a fixed-dimension
//! deterministic placeholder vector.

/// Produces fixed-dimension embedding vectors for text.
#[derive(Debug, Clone)]
pub struct Embedder {
    dim: usize,
}

impl Embedder {
    /// Create an embedder producing vectors of `dim` floats.
    pub fn new(dim: usize) -> Self {
        Self { dim }
    }

    /// Embedding dimensionality.
    pub fn dim(&self) -> usize {
        self.dim
    }

    /// Deterministic placeholder embedding (M1). Replaced by a real model in M3.
    pub fn embed(&self, text: &str) -> Vec<f32> {
        let seed = text.len() as f32;
        (0..self.dim)
            .map(|i| (seed * 0.1) + (i as f32 * 0.01))
            .collect()
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
}
