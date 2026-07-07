//! Codec parity tests for int8 embedding quantization (WS2 spike, issue #17).
//!
//! These exercise the codec against vectors from the crate's own [`Embedder`]
//! (the same embeddings the pack pipeline produces) and against dense unit
//! vectors representative of the reference BGE fp32 model. They assert the
//! retrieval-relevant contract: direction (cosine) is preserved, per-element
//! error is bounded by the scale, the additive `QuantizedEmbedding` unit round
//! trips, corrupt inputs are rejected, and the format actually shrinks storage.
//!
//! The *adoption* decision (enabling quantization for real packs) is gated on
//! the WS1 #16 eval recall-parity baseline and is intentionally left DISABLED;
//! [`quantization_enabled`] is asserted `false` here to lock that state.

use kgpacks_embeddings::{
    compression_ratio, dequantize_int8, dequantize_int8_dim, fp32_bytes, int8_bytes,
    quantization_enabled, quantize_int8, Embedder, QuantizeError, QuantizedEmbedding,
};

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    dot / (na * nb)
}

fn sample_texts() -> Vec<&'static str> {
    vec![
        "CVE-2024-3094 is a backdoor in the xz-utils compression library",
        "Remote code execution vulnerability affecting OpenSSL 3.0 servers",
        "A privilege escalation flaw in the Linux kernel netfilter subsystem",
        "Cross-site scripting in a popular JavaScript templating framework",
        "Deserialization of untrusted data leads to arbitrary code execution",
    ]
}

/// A dense unit vector, representative of a real BGE fp32 embedding.
fn dense_unit_vector(dim: usize, seed: u64) -> Vec<f32> {
    let mut state = seed | 1;
    let mut v: Vec<f32> = (0..dim)
        .map(|_| {
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            let bits = state.wrapping_mul(0x2545_f491_4f6c_dd1d);
            (bits as f64 / u64::MAX as f64) as f32 * 2.0 - 1.0
        })
        .collect();
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    for x in &mut v {
        *x /= norm;
    }
    v
}

#[test]
fn quantization_stays_disabled_pending_ws1_16() {
    let enabled = quantization_enabled();
    assert!(
        !enabled,
        "adoption is gated on WS1 #16 eval recall parity; keep the flag disabled"
    );
}

#[test]
fn preserves_cosine_on_embedder_vectors() {
    let embedder = Embedder::bge();
    for text in sample_texts() {
        let v = embedder.embed(text);
        let (codes, scale) = quantize_int8(&v);
        let round = dequantize_int8(&codes, scale).unwrap();
        let c = cosine(&v, &round);
        assert!(
            c > 0.999,
            "cosine {c} below 0.999 for embedder vector of {text:?}"
        );
    }
}

#[test]
fn preserves_cosine_on_dense_bge_like_vectors() {
    for seed in [3_u64, 11, 97, 2024, 2025] {
        let v = dense_unit_vector(768, seed);
        let (codes, scale) = quantize_int8(&v);
        let round = dequantize_int8(&codes, scale).unwrap();
        assert!(cosine(&v, &round) > 0.999);
    }
}

#[test]
fn preserves_relative_ranking_of_neighbours() {
    // Quantization must not reorder nearest neighbours: the fp32 ranking of a
    // query against a small corpus should survive the round trip.
    let embedder = Embedder::bge();
    let texts = sample_texts();
    let corpus: Vec<Vec<f32>> = texts.iter().map(|t| embedder.embed(t)).collect();
    let query = embedder
        .generate_query(&["remote code execution in OpenSSL"])
        .unwrap()[0]
        .clone();

    let mut fp32_order: Vec<usize> = (0..corpus.len()).collect();
    fp32_order.sort_by(|&a, &b| {
        cosine(&query, &corpus[b])
            .partial_cmp(&cosine(&query, &corpus[a]))
            .unwrap()
    });

    let quantized: Vec<Vec<f32>> = corpus
        .iter()
        .map(|v| {
            let (codes, scale) = quantize_int8(v);
            dequantize_int8(&codes, scale).unwrap()
        })
        .collect();
    let mut q_order: Vec<usize> = (0..quantized.len()).collect();
    q_order.sort_by(|&a, &b| {
        cosine(&query, &quantized[b])
            .partial_cmp(&cosine(&query, &quantized[a]))
            .unwrap()
    });

    assert_eq!(
        fp32_order, q_order,
        "int8 quantization reordered nearest neighbours"
    );
}

#[test]
fn per_element_error_bounded_by_scale() {
    let embedder = Embedder::bge();
    for text in sample_texts() {
        let v = embedder.embed(text);
        let (codes, scale) = quantize_int8(&v);
        let round = dequantize_int8(&codes, scale).unwrap();
        for (orig, approx) in v.iter().zip(&round) {
            assert!((orig - approx).abs() <= scale + 1e-6);
        }
    }
}

#[test]
fn additive_unit_round_trips_and_reports_size() {
    let v = dense_unit_vector(768, 42);
    let q = QuantizedEmbedding::encode(&v);
    assert_eq!(q.dim(), 768);
    assert_eq!(q.encoded_bytes(), int8_bytes(768));
    assert!(q.encoded_bytes() < fp32_bytes(768));
    let round = q.decode().unwrap();
    assert!(cosine(&v, &round) > 0.999);
}

#[test]
fn corrupt_pack_inputs_are_rejected_not_panicked() {
    // Wrong scale.
    assert_eq!(
        dequantize_int8(&[1, 2, 3], f32::NAN),
        Err(QuantizeError::NonFiniteScale)
    );
    assert_eq!(
        dequantize_int8(&[1, 2, 3], -1.0),
        Err(QuantizeError::NegativeScale)
    );

    // Wrong length via the dimensioned decode path: a valid-looking code buffer
    // whose length disagrees with the declared dimension is rejected (not a
    // panic), mirroring a truncated/corrupt pack.
    let q = QuantizedEmbedding::encode(&dense_unit_vector(64, 7));
    let codes = q.codes();
    assert_eq!(
        dequantize_int8_dim(&codes[..codes.len() - 1], q.scale(), q.dim()),
        Err(QuantizeError::LengthMismatch {
            expected: 64,
            actual: 63,
        })
    );
    // The matching-length buffer decodes cleanly.
    assert!(dequantize_int8_dim(codes, q.scale(), q.dim()).is_ok());
}

#[test]
fn shrinks_storage_by_about_four_x() {
    let ratio = compression_ratio(768);
    assert!(
        ratio > 3.9 && ratio < 4.0,
        "expected ~3.97x shrink at 768-d, got {ratio}"
    );
}
