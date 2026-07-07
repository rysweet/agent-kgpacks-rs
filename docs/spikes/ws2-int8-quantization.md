# WS2 spike: int8 embedding quantization (issue #17)

**Status:** codec implemented and tested; **adoption DISABLED** pending the WS1
#16 eval recall-parity baseline.

## Goal

The reference CVE pack stores fp32 BGE embeddings (768-d), ~2.1 GB. This spike
evaluates **scalar int8 quantization** to shrink the pack and gates adoption on a
recall-parity check via the eval harness. The feature ships behind a flag **only
if** parity holds.

## What landed in this spike

A self-contained, fully-tested codec in the `kgpacks-embeddings` crate
([`quant`](../../crates/kgpacks-embeddings/src/quant.rs)). It is **not** wired
into pack building — that is the adoption step, which is gated (see below).

* `quantize_int8(v: &[f32]) -> (Vec<i8>, f32)` — `scale = max(|v_i|) / 127`;
  all-zero/empty input yields `scale = 0.0` (no `0/0` NaN); non-finite inputs do
  not poison the scale and quantize to code `0`.
* `dequantize_int8(codes, scale) -> Result<Vec<f32>, QuantizeError>` — bound
  checked: rejects non-finite and negative scales.
* `dequantize_int8_dim(codes, scale, expected_dim)` — additionally rejects a
  wrong-length code buffer (the pack-decode path).
* `QuantizedEmbedding { dim, scale, codes }` — the **additive** on-pack unit that
  would replace a `FLOAT[dim]` column *when adopted*, carrying its own `dim` for
  independent bound-checking. Existing fp32 packs are untouched.
* `quantization_enabled() -> bool` — the adoption flag, hard-wired `false`.

### Format & size

| representation        | bytes per 768-d vector | note                         |
| --------------------- | ---------------------- | ---------------------------- |
| fp32 (`FLOAT[768]`)   | 3072                   | current                      |
| int8 + scale          | 772                    | 768 × `i8` + one `f32` scale |

Shrink factor ≈ **3.97×** at 768-d (`fp32_bytes / int8_bytes`), projecting the
~2.1 GB embedding payload toward ~0.53 GB. The format is **additive**: a new
optional quantized unit, chosen per-pack, that never breaks existing fp32 packs.

### Measured codec quality (unit + integration tests)

* **Round-trip error** per element `≤ scale` (in practice `≤ scale/2`).
* **Cosine preserved** `> 0.999` between each L2-normalized vector and its round
  trip — verified on both the crate's `Embedder` vectors and dense BGE-like unit
  vectors.
* **Nearest-neighbour ranking preserved** on a small CVE corpus (quantization did
  not reorder neighbours).
* **No NaN / no panics** on all-zero, empty, non-finite, and corrupt inputs.

Tests: `crates/kgpacks-embeddings/src/quant.rs` (unit) and
`crates/kgpacks-embeddings/tests/quant.rs` (integration). Run with
`cargo test -p kgpacks-embeddings`.

## Why adoption is DISABLED (dependency: WS1 #16)

Issue #17's done-criterion is **"gated on eval recall parity"**: adopt only if
`delta_accuracy >= -0.02` **and** retrieval hit@k parity holds against the WS1
baseline. That baseline is produced by **WS1 #16** (full-pack CVE eval
validation + extended real 2024/2025 eval questions), which is still **open** —
the `kgpacks-eval` harness is currently an M1 substring-match scaffold, so a real
recall-parity number **cannot be measured yet**.

Per the issue's own instruction — *"Otherwise leave the feature DISABLED and
commit spike findings"* — this spike ships the codec disabled rather than
enabling an unverified format. This keeps the low-risk, dependency-free work
landable now while respecting the hard dependency.

## Re-evaluation checklist (do this once WS1 #16 lands)

1. Build a quantized copy of the CVE pack using `QuantizedEmbedding::encode`.
2. Run the #16 eval harness on fp32 vs quantized; record `delta_accuracy` and
   hit@k.
3. If `delta_accuracy >= -0.02` and hit@k parity holds → flip
   `quantization_enabled()` to `true`, wire the additive unit into pack
   build/load, and document the parity result.
4. Otherwise → keep disabled and record the observed deltas here.

## Out of scope for this spike

Product quantization (PQ), referenced in the issue title, is a heavier
codebook-based approach. The issue's concrete scope and acceptance criteria
specify the **int8 scalar** codec; PQ is left as a follow-up to evaluate only if
int8 fails the parity gate or more aggressive compression is needed.
