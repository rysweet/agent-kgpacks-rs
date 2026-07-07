//! Int8 (scalar) quantization codec for embedding vectors.
//!
//! **WS2 spike ([issue #17]).** The reference CVE pack stores fp32 BGE
//! embeddings (~2.1 GB). Scalar int8 quantization stores each vector as one
//! `f32` scale plus one `i8` per dimension, cutting the per-vector footprint
//! from `4·dim` bytes to `dim + 4` bytes — a ~3.97× shrink at `dim == 768`
//! ([`compression_ratio`]).
//!
//! # Spike status: DISABLED, pending eval recall parity (WS1 #16)
//!
//! Adopting quantization for real packs is **gated on a recall-parity check**
//! run through the WS1 eval harness (issue #16). That baseline has not landed,
//! so this codec is **not** wired into pack building and the feature is reported
//! as disabled by [`quantization_enabled`]. The math and its round-trip
//! guarantees are fully implemented and unit-tested here so the parity gate can
//! be evaluated the moment #16's baseline exists; only *enabling* the flag waits
//! on that measurement. See `docs/spikes/ws2-int8-quantization.md`.
//!
//! # Codec contract
//!
//! Symmetric per-vector quantization: `scale = max(|v_i|) / 127`, then
//! `code_i = round(v_i / scale)` clamped to `[-127, 127]` (the `i8` value
//! `-128` is never emitted, so the range is symmetric and `0.0` always maps to
//! code `0`). Reconstruction is `v_i ≈ code_i · scale`.
//!
//! Guarantees (see the tests):
//!
//! * **No NaN.** An all-zero (or empty) vector yields `scale == 0.0`, never a
//!   `0/0` NaN; non-finite input elements are ignored when picking the scale and
//!   quantize to code `0`.
//! * **Bounded error.** Per-element reconstruction error is `≤ scale`
//!   (in fact `≤ scale/2`).
//! * **Direction preserved.** Cosine similarity between an L2-normalized vector
//!   and its round-trip stays `> 0.999`.
//!
//! [issue #17]: https://github.com/rysweet/agent-kgpacks-rs/issues/17

/// Largest magnitude an emitted `i8` code takes, keeping the code range
/// symmetric (`[-127, 127]`, so `-128` is never produced).
pub const INT8_CODE_MAX: f32 = 127.0;

/// Whether int8 embedding quantization is adopted for real packs.
///
/// Hard-wired `false` for the WS2 spike: adoption is gated on the WS1 #16 eval
/// recall-parity baseline, which has not landed. The codec below is fully
/// functional regardless; this only reports whether packs should *use* it.
#[must_use]
pub const fn quantization_enabled() -> bool {
    false
}

/// Error returned by the fallible codec entry points.
#[derive(Debug, Clone, PartialEq)]
pub enum QuantizeError {
    /// The `scale` was `NaN` or `±∞`; a finite scale is required to reconstruct.
    NonFiniteScale,
    /// The `scale` was negative; scales are magnitudes and must be `≥ 0`.
    NegativeScale,
    /// The code slice length did not match the expected vector dimension.
    LengthMismatch {
        /// Dimension the caller asked to reconstruct.
        expected: usize,
        /// Number of codes actually supplied.
        actual: usize,
    },
}

impl std::fmt::Display for QuantizeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QuantizeError::NonFiniteScale => write!(f, "quantization scale must be finite"),
            QuantizeError::NegativeScale => write!(f, "quantization scale must be non-negative"),
            QuantizeError::LengthMismatch { expected, actual } => write!(
                f,
                "quantized code length {actual} does not match expected dimension {expected}"
            ),
        }
    }
}

impl std::error::Error for QuantizeError {}

/// Quantize an fp32 vector to `i8` codes plus a shared `f32` scale.
///
/// `scale = max(|v_i|) / 127`. An all-zero or empty vector returns all-zero
/// codes with `scale == 0.0` (no `0/0` NaN). Non-finite input elements do not
/// influence the scale and quantize to code `0`, so the result is always
/// well-formed. Reconstruct with [`dequantize_int8`].
#[must_use]
pub fn quantize_int8(v: &[f32]) -> (Vec<i8>, f32) {
    // Ignore non-finite elements when picking the scale so a stray NaN/∞ can
    // never poison it into a NaN (which would break every reconstruction).
    let max_abs = v
        .iter()
        .copied()
        .filter(|x| x.is_finite())
        .map(f32::abs)
        .fold(0.0_f32, f32::max);

    if max_abs == 0.0 {
        // All-zero (or all-non-finite / empty): scale 0, codes 0. Encodes the
        // zero vector exactly and keeps dequantization NaN-free.
        return (vec![0_i8; v.len()], 0.0);
    }

    let scale = max_abs / INT8_CODE_MAX;
    let codes = v
        .iter()
        .map(|&x| {
            if !x.is_finite() {
                return 0_i8;
            }
            // `f32 as i8` already saturates, but clamp first so the range stays
            // symmetric ([-127, 127]) and the cast is always exact.
            (x / scale).round().clamp(-INT8_CODE_MAX, INT8_CODE_MAX) as i8
        })
        .collect();
    (codes, scale)
}

/// Reconstruct an fp32 vector from `i8` codes and a shared `scale`.
///
/// Returns [`QuantizeError::NonFiniteScale`] for a `NaN`/`±∞` scale and
/// [`QuantizeError::NegativeScale`] for a negative scale. The output length
/// equals `codes.len()`; use [`dequantize_int8_dim`] to also assert a specific
/// target dimension.
pub fn dequantize_int8(codes: &[i8], scale: f32) -> Result<Vec<f32>, QuantizeError> {
    validate_scale(scale)?;
    Ok(codes.iter().map(|&c| f32::from(c) * scale).collect())
}

/// Like [`dequantize_int8`], but also rejects a code slice whose length does not
/// match `expected_dim` (the pack-decode path, where the dimension is known).
pub fn dequantize_int8_dim(
    codes: &[i8],
    scale: f32,
    expected_dim: usize,
) -> Result<Vec<f32>, QuantizeError> {
    if codes.len() != expected_dim {
        return Err(QuantizeError::LengthMismatch {
            expected: expected_dim,
            actual: codes.len(),
        });
    }
    dequantize_int8(codes, scale)
}

fn validate_scale(scale: f32) -> Result<(), QuantizeError> {
    if !scale.is_finite() {
        return Err(QuantizeError::NonFiniteScale);
    }
    if scale < 0.0 {
        return Err(QuantizeError::NegativeScale);
    }
    Ok(())
}

/// Bytes needed to store a `dim`-length fp32 vector.
#[must_use]
pub const fn fp32_bytes(dim: usize) -> usize {
    dim * std::mem::size_of::<f32>()
}

/// Bytes needed to store a `dim`-length int8-quantized vector (`dim` codes plus
/// one `f32` scale).
#[must_use]
pub const fn int8_bytes(dim: usize) -> usize {
    dim * std::mem::size_of::<i8>() + std::mem::size_of::<f32>()
}

/// Storage shrink factor of int8 vs fp32 at dimension `dim`
/// (`fp32_bytes / int8_bytes`). `0.0` for `dim == 0`.
#[must_use]
pub fn compression_ratio(dim: usize) -> f32 {
    if dim == 0 {
        return 0.0;
    }
    fp32_bytes(dim) as f32 / int8_bytes(dim) as f32
}

/// An int8-quantized embedding: the additive on-pack unit that replaces a
/// `FLOAT[dim]` column when (and only when) quantization is adopted.
///
/// Carries its own `dim` so a decoder can bound-check the stored codes
/// independently of the surrounding schema; existing fp32 packs are untouched.
#[derive(Debug, Clone, PartialEq)]
pub struct QuantizedEmbedding {
    dim: usize,
    scale: f32,
    codes: Vec<i8>,
}

impl QuantizedEmbedding {
    /// Quantize an fp32 embedding into its int8 form.
    #[must_use]
    pub fn encode(v: &[f32]) -> Self {
        let (codes, scale) = quantize_int8(v);
        Self {
            dim: v.len(),
            scale,
            codes,
        }
    }

    /// Reconstruct the approximate fp32 embedding.
    ///
    /// Returns an error if the stored `scale`/`codes` are inconsistent
    /// (non-finite/negative scale, or `codes.len() != dim` — e.g. from a
    /// corrupt pack).
    pub fn decode(&self) -> Result<Vec<f32>, QuantizeError> {
        dequantize_int8_dim(&self.codes, self.scale, self.dim)
    }

    /// Embedding dimensionality.
    #[must_use]
    pub fn dim(&self) -> usize {
        self.dim
    }

    /// Shared reconstruction scale.
    #[must_use]
    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// The raw int8 codes.
    #[must_use]
    pub fn codes(&self) -> &[i8] {
        &self.codes
    }

    /// Encoded footprint in bytes (`dim` code bytes + one `f32` scale).
    #[must_use]
    pub fn encoded_bytes(&self) -> usize {
        int8_bytes(self.dim)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cosine(a: &[f32], b: &[f32]) -> f32 {
        let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
        let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        dot / (na * nb)
    }

    /// Deterministic dense unit vector, representative of a real (dense) BGE
    /// fp32 embedding rather than the sparse hashed embedder used elsewhere.
    fn dense_unit_vector(dim: usize, seed: u64) -> Vec<f32> {
        let mut state = seed | 1;
        let mut v: Vec<f32> = (0..dim)
            .map(|_| {
                // xorshift64* -> value in (-1, 1)
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
    fn flag_disabled_until_parity_lands() {
        // Lock the spike's "leave DISABLED until #16 parity" done-criterion.
        let enabled = quantization_enabled();
        assert!(
            !enabled,
            "int8 quantization must stay disabled until WS1 #16 recall parity is measured"
        );
    }

    #[test]
    fn scale_is_max_abs_over_127() {
        let v = [0.0, -2.0, 1.0, 0.5];
        let (_, scale) = quantize_int8(&v);
        assert!((scale - 2.0 / 127.0).abs() < 1e-9);
    }

    #[test]
    fn max_magnitude_element_uses_full_range() {
        let v = [0.0, -2.0, 1.0, 0.5];
        let (codes, _) = quantize_int8(&v);
        // The largest-|·| element saturates the symmetric range at ∓127.
        assert_eq!(codes[1], -127);
        // Codes stay within the symmetric range (never -128).
        assert!(codes.iter().all(|&c| c >= -127));
    }

    #[test]
    fn all_zero_vector_has_zero_scale_no_nan() {
        let (codes, scale) = quantize_int8(&[0.0, 0.0, 0.0]);
        assert_eq!(scale, 0.0);
        assert_eq!(codes, vec![0, 0, 0]);
        let round = dequantize_int8(&codes, scale).unwrap();
        assert_eq!(round, vec![0.0, 0.0, 0.0]);
        assert!(round.iter().all(|x| x.is_finite()));
    }

    #[test]
    fn empty_vector_round_trips() {
        let (codes, scale) = quantize_int8(&[]);
        assert!(codes.is_empty());
        assert_eq!(scale, 0.0);
        assert_eq!(dequantize_int8(&codes, scale).unwrap(), Vec::<f32>::new());
    }

    #[test]
    fn non_finite_input_does_not_poison_scale() {
        let v = [f32::NAN, 1.0, f32::INFINITY, -0.5];
        let (codes, scale) = quantize_int8(&v);
        assert!(scale.is_finite());
        // max finite magnitude is 1.0 -> scale = 1/127
        assert!((scale - 1.0 / 127.0).abs() < 1e-9);
        // Non-finite elements quantize to 0.
        assert_eq!(codes[0], 0);
        assert_eq!(codes[2], 0);
        let round = dequantize_int8(&codes, scale).unwrap();
        assert!(round.iter().all(|x| x.is_finite()));
    }

    #[test]
    fn round_trip_error_per_element_within_scale() {
        for seed in [1_u64, 7, 42, 1234, 999_999] {
            let v = dense_unit_vector(256, seed);
            let (codes, scale) = quantize_int8(&v);
            let round = dequantize_int8(&codes, scale).unwrap();
            for (orig, approx) in v.iter().zip(&round) {
                assert!(
                    (orig - approx).abs() <= scale + 1e-6,
                    "per-element error {} exceeded scale {scale}",
                    (orig - approx).abs()
                );
            }
        }
    }

    #[test]
    fn cosine_preserved_above_0_999() {
        for seed in [1_u64, 7, 42, 1234, 999_999] {
            let v = dense_unit_vector(768, seed);
            let (codes, scale) = quantize_int8(&v);
            let round = dequantize_int8(&codes, scale).unwrap();
            let c = cosine(&v, &round);
            assert!(c > 0.999, "cosine {c} for seed {seed} fell below 0.999");
        }
    }

    #[test]
    fn dequantize_rejects_non_finite_scale() {
        let codes = [1_i8, -2, 3];
        assert_eq!(
            dequantize_int8(&codes, f32::NAN),
            Err(QuantizeError::NonFiniteScale)
        );
        assert_eq!(
            dequantize_int8(&codes, f32::INFINITY),
            Err(QuantizeError::NonFiniteScale)
        );
    }

    #[test]
    fn dequantize_rejects_negative_scale() {
        assert_eq!(
            dequantize_int8(&[1_i8, 2, 3], -0.5),
            Err(QuantizeError::NegativeScale)
        );
    }

    #[test]
    fn dequantize_dim_rejects_wrong_length() {
        let codes = [1_i8, 2, 3];
        assert_eq!(
            dequantize_int8_dim(&codes, 0.1, 4),
            Err(QuantizeError::LengthMismatch {
                expected: 4,
                actual: 3
            })
        );
        // Correct length passes.
        assert!(dequantize_int8_dim(&codes, 0.1, 3).is_ok());
    }

    #[test]
    fn quantized_embedding_round_trip_and_bound_check() {
        let v = dense_unit_vector(768, 2024);
        let q = QuantizedEmbedding::encode(&v);
        assert_eq!(q.dim(), 768);
        assert_eq!(q.codes().len(), 768);
        let round = q.decode().unwrap();
        assert!(cosine(&v, &round) > 0.999);

        // A corrupt (wrong-length) code buffer is rejected on decode.
        let corrupt = QuantizedEmbedding {
            dim: 768,
            scale: q.scale(),
            codes: q.codes()[..767].to_vec(),
        };
        assert!(matches!(
            corrupt.decode(),
            Err(QuantizeError::LengthMismatch { .. })
        ));
    }

    #[test]
    fn compression_ratio_shrinks_storage() {
        assert_eq!(fp32_bytes(768), 3072);
        assert_eq!(int8_bytes(768), 772);
        let ratio = compression_ratio(768);
        assert!(ratio > 3.9 && ratio < 4.0, "unexpected ratio {ratio}");
        assert_eq!(compression_ratio(0), 0.0);
        assert_eq!(
            QuantizedEmbedding::encode(&dense_unit_vector(768, 1)).encoded_bytes(),
            772
        );
    }
}
