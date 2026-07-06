//! Multi-part release index — the split/accounting half of the pack release
//! tool, exercised in "dry-run" (compute the index; publish nothing).
//!
//! A pack that exceeds [`MAX_SINGLE_ARTIFACT_BYTES`] (2 GiB) is published as an
//! ordered set of fixed-size parts. The index below is the manifest a
//! `pack pull` re-verifies before reassembling the artifact:
//!
//! * every non-final part is exactly `part_size` bytes; the final part is the
//!   remainder (`1..=part_size`),
//! * `sum(parts.bytes) == total_bytes`,
//! * each part carries the SHA-256 of *its own* bytes, and
//! * the index carries the SHA-256 of the whole artifact (== the hash of the
//!   parts concatenated in order).
//!
//! [`plan_multipart_release`] is the single source of truth for that format, so
//! a test can drive the *real* planner over a tiny synthetic pack rather than
//! hand-rolling an index that could drift from what pull verifies.
//!
//! Size accounting ([`part_accounting`]) is pure `u64` arithmetic that never
//! materializes the artifact, so the >2 GiB path is unit-testable without
//! allocating gigabytes.

use crate::errors::{PacksError, Result};
use crate::sha256::sha256_hex;

/// Above this single-artifact size (2 GiB), a pack MUST be published as a
/// multi-part release rather than a single blob.
pub const MAX_SINGLE_ARTIFACT_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Whether an artifact of `total_bytes` must be split into multiple parts.
pub fn requires_multipart(total_bytes: u64) -> bool {
    total_bytes > MAX_SINGLE_ARTIFACT_BYTES
}

/// Compact, allocation-free accounting for splitting `total_bytes` into
/// `part_size` chunks.
///
/// This is the >2 GiB-safe path: it computes part counts and the final-part
/// size with `u64` arithmetic and holds no artifact bytes, so it is valid for
/// multi-gigabyte totals (and for tiny `part_size` values that would imply
/// billions of parts — no per-part vector is allocated).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartAccounting {
    /// Total artifact size in bytes.
    pub total_bytes: u64,
    /// Fixed size of every non-final part.
    pub part_size: u64,
    /// Number of parts (`0` iff `total_bytes == 0`).
    pub num_parts: u64,
    /// Size of the final part (`1..=part_size`, or `0` iff `total_bytes == 0`).
    pub last_part_bytes: u64,
}

impl PartAccounting {
    /// The byte size of part `index` (0-based). Returns `None` if out of range.
    pub fn part_bytes(&self, index: u64) -> Option<u64> {
        if index >= self.num_parts {
            return None;
        }
        Some(if index + 1 == self.num_parts {
            self.last_part_bytes
        } else {
            self.part_size
        })
    }
}

/// Compute [`PartAccounting`] for `total_bytes` split into `part_size` chunks.
///
/// Errors ([`PacksError::PackInstall`]) if `part_size == 0`.
pub fn part_accounting(total_bytes: u64, part_size: u64) -> Result<PartAccounting> {
    if part_size == 0 {
        return Err(PacksError::PackInstall(
            "part_size must be greater than zero".into(),
        ));
    }
    if total_bytes == 0 {
        return Ok(PartAccounting {
            total_bytes: 0,
            part_size,
            num_parts: 0,
            last_part_bytes: 0,
        });
    }
    // Ceiling division without `total_bytes + part_size` overflow.
    let num_parts = (total_bytes - 1) / part_size + 1;
    let last_part_bytes = total_bytes - (num_parts - 1) * part_size;
    Ok(PartAccounting {
        total_bytes,
        part_size,
        num_parts,
        last_part_bytes,
    })
}

/// One part of a multi-part pack release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartEntry {
    /// 0-based ordinal of this part.
    pub index: u64,
    /// Size of this part in bytes.
    pub bytes: u64,
    /// Lowercase-hex SHA-256 of this part's bytes.
    pub sha256: String,
}

/// The multi-part release index for a single pack artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultiPartIndex {
    /// Fixed size of every non-final part.
    pub part_size: u64,
    /// Total artifact size (== `sum(parts.bytes)`).
    pub total_bytes: u64,
    /// Lowercase-hex SHA-256 of the whole artifact (== hash of the parts
    /// concatenated in order).
    pub sha256: String,
    /// The parts, in order.
    pub parts: Vec<PartEntry>,
}

impl MultiPartIndex {
    /// Serialize to the canonical `<name>.pack-release.json` multi-part shape.
    ///
    /// This is the exact structure `pack pull` verifies against; the planner
    /// and the pull-side reader share it so the format cannot drift.
    pub fn to_value(&self) -> serde_json::Value {
        serde_json::json!({
            "part_size": self.part_size,
            "total_bytes": self.total_bytes,
            "sha256": self.sha256,
            "parts": self
                .parts
                .iter()
                .map(|p| serde_json::json!({
                    "index": p.index,
                    "bytes": p.bytes,
                    "sha256": p.sha256,
                }))
                .collect::<Vec<_>>(),
        })
    }
}

/// Plan a multi-part release over `data` split into `part_size`-byte chunks —
/// the real release-index computation, run dry (nothing is published).
///
/// Computes the per-part and overall SHA-256 digests so the returned
/// [`MultiPartIndex`] is byte-for-byte what a publish would record. Errors
/// ([`PacksError::PackInstall`]) if `part_size == 0`.
pub fn plan_multipart_release(data: &[u8], part_size: u64) -> Result<MultiPartIndex> {
    let accounting = part_accounting(data.len() as u64, part_size)?;

    let chunk = usize::try_from(part_size).map_err(|_| {
        PacksError::PackInstall("part_size does not fit this platform's usize".into())
    })?;

    let mut parts = Vec::with_capacity(accounting.num_parts as usize);
    for (index, slice) in data.chunks(chunk).enumerate() {
        parts.push(PartEntry {
            index: index as u64,
            bytes: slice.len() as u64,
            sha256: sha256_hex(slice),
        });
    }

    Ok(MultiPartIndex {
        part_size,
        total_bytes: data.len() as u64,
        sha256: sha256_hex(data),
        parts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accounting_exact_multiple() {
        let a = part_accounting(10, 5).unwrap();
        assert_eq!(a.num_parts, 2);
        assert_eq!(a.last_part_bytes, 5);
        assert_eq!(a.part_bytes(0), Some(5));
        assert_eq!(a.part_bytes(1), Some(5));
        assert_eq!(a.part_bytes(2), None);
    }

    #[test]
    fn accounting_with_remainder() {
        let a = part_accounting(11, 5).unwrap();
        assert_eq!(a.num_parts, 3);
        assert_eq!(a.last_part_bytes, 1);
        assert_eq!(a.part_bytes(2), Some(1));
    }

    #[test]
    fn accounting_zero_total() {
        let a = part_accounting(0, 5).unwrap();
        assert_eq!(a.num_parts, 0);
        assert_eq!(a.last_part_bytes, 0);
        assert_eq!(a.part_bytes(0), None);
    }

    #[test]
    fn accounting_rejects_zero_part_size() {
        assert!(part_accounting(10, 0).is_err());
    }
}
