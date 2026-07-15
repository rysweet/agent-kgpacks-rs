//! WS5 — multi-part release accounting coverage.
//!
//! Drives the **real** release-index planner
//! ([`kgpacks_packs::plan_multipart_release`]) so the on-disk multi-part index
//! format cannot drift from what a `pack pull` re-verifies, and unit-tests the
//! >2 GiB size accounting without ever materializing >2 GiB of bytes.

use kgpacks_packs::{
    build_release_index, pack_part_filename, part_accounting, plan_multipart_release,
    requires_multipart, sha256_hex, PackReleaseIndex, ProvenanceOverrides,
    MAX_SINGLE_ARTIFACT_BYTES,
};

/// Deterministic, high-entropy ("incompressible") bytes — a stand-in for a real
/// packed artifact, generated with a tiny xorshift PRNG so the test needs no
/// `rand` dependency and is reproducible in CI.
fn pseudo_random_bytes(len: usize, seed: u64) -> Vec<u8> {
    let mut state = seed | 1;
    let mut out = Vec::with_capacity(len);
    for _ in 0..len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        out.push((state >> 33) as u8);
    }
    out
}

#[test]
fn multipart_split_accounting_holds_over_a_synthetic_pack() {
    // A tiny 1 KiB part size over a ~4.6 KiB synthetic pack forces a genuine
    // multi-part split with a smaller final part.
    const PART_SIZE: u64 = 1024;
    let data = pseudo_random_bytes(4703, 0xC0FFEE);

    let index = plan_multipart_release(&data, PART_SIZE).expect("plan release");

    // A genuine multi-part split.
    assert!(
        index.parts.len() > 1,
        "expected >1 part, got {}",
        index.parts.len()
    );
    assert_eq!(index.part_size, PART_SIZE);

    // Every non-final part is exactly the part size; the final part is the
    // (smaller, non-zero) remainder.
    let last = index.parts.len() - 1;
    for (i, part) in index.parts.iter().enumerate() {
        assert_eq!(part.index, i as u64, "parts must be ordered 0..n");
        if i < last {
            assert_eq!(
                part.bytes, PART_SIZE,
                "non-final part {i} must equal part_size"
            );
        } else {
            assert!(
                part.bytes >= 1 && part.bytes <= PART_SIZE,
                "final part in 1..=part_size"
            );
            assert_ne!(
                part.bytes, PART_SIZE,
                "this fixture's final part is a remainder"
            );
        }
    }

    // sum(parts.bytes) == total_bytes == the artifact length.
    let summed: u64 = index.parts.iter().map(|p| p.bytes).sum();
    assert_eq!(summed, index.total_bytes);
    assert_eq!(index.total_bytes, data.len() as u64);

    // Per-part sha256 matches that part's own bytes, and reassembling the parts
    // reproduces the artifact byte-for-byte.
    let mut reassembled = Vec::with_capacity(data.len());
    let mut offset = 0usize;
    for part in &index.parts {
        let slice = &data[offset..offset + part.bytes as usize];
        assert_eq!(part.sha256, sha256_hex(slice), "part {} hash", part.index);
        reassembled.extend_from_slice(slice);
        offset += part.bytes as usize;
    }
    assert_eq!(
        reassembled, data,
        "parts must reassemble to the original artifact"
    );

    // The overall digest is the hash of the concatenated parts (== the artifact).
    assert_eq!(index.sha256, sha256_hex(&reassembled));
    assert_eq!(index.sha256, sha256_hex(&data));

    // The serialized index carries exactly the fields pull re-verifies.
    let value = index.to_value();
    assert_eq!(value["part_size"], serde_json::json!(PART_SIZE));
    assert_eq!(value["total_bytes"], serde_json::json!(data.len() as u64));
    assert_eq!(value["sha256"], serde_json::json!(index.sha256));
    assert_eq!(
        value["parts"].as_array().map(|a| a.len()),
        Some(index.parts.len())
    );
}

#[test]
fn single_part_when_data_fits_in_one_chunk() {
    let data = pseudo_random_bytes(500, 7);
    let index = plan_multipart_release(&data, 1024).expect("plan");
    assert_eq!(index.parts.len(), 1);
    assert_eq!(index.parts[0].bytes, 500);
    assert_eq!(index.sha256, sha256_hex(&data));
}

#[test]
fn plan_rejects_zero_part_size() {
    assert!(plan_multipart_release(b"anything", 0).is_err());
}

#[test]
fn accounts_for_over_2gib_without_materializing() {
    // Exactly 3 GiB in 64 MiB parts: 48 full parts, no remainder. Pure u64
    // accounting — nothing is allocated, so this is safe for multi-GiB totals.
    const GIB: u64 = 1024 * 1024 * 1024;
    const PART_SIZE: u64 = 64 * 1024 * 1024; // 64 MiB
    let total = 3 * GIB;

    assert!(total > MAX_SINGLE_ARTIFACT_BYTES);
    assert!(requires_multipart(total));
    assert!(!requires_multipart(MAX_SINGLE_ARTIFACT_BYTES));
    assert!(requires_multipart(MAX_SINGLE_ARTIFACT_BYTES + 1));

    let a = part_accounting(total, PART_SIZE).expect("accounting");
    assert_eq!(a.num_parts, 48);
    assert_eq!(a.last_part_bytes, PART_SIZE);

    // Reconstruct the total from the accounting: (n-1) full parts + final part.
    let reconstructed = (a.num_parts - 1) * a.part_size + a.last_part_bytes;
    assert_eq!(reconstructed, total);

    // Summing every part via the accessor also recovers the total exactly.
    let summed: u64 = (0..a.num_parts).map(|i| a.part_bytes(i).unwrap()).sum();
    assert_eq!(summed, total);
    assert_eq!(a.part_bytes(a.num_parts), None);
}

#[test]
fn over_2gib_with_remainder_accounts_the_final_part() {
    const GIB: u64 = 1024 * 1024 * 1024;
    const PART_SIZE: u64 = 64 * 1024 * 1024;
    let total = 3 * GIB + 1000; // one extra tiny part

    let a = part_accounting(total, PART_SIZE).expect("accounting");
    assert_eq!(a.num_parts, 49);
    assert_eq!(a.last_part_bytes, 1000);
    assert_eq!((a.num_parts - 1) * a.part_size + a.last_part_bytes, total);
    assert!(a.total_bytes > MAX_SINGLE_ARTIFACT_BYTES);
}

/// Anti-drift guard: the planner's per-part accounting and the on-disk
/// `<name>.pack-release.json` that `pack pull` reads MUST agree.
///
/// The WS5 acceptance requires driving the real release tool so "the index
/// format can't drift from what `pack pull` verifies." The planner
/// ([`plan_multipart_release`] → `MultiPartIndex`) and the pull-facing index
/// ([`PackReleaseIndex`], built by [`build_release_index`] and written to the
/// `<name>.pack-release.json` `pack pull` reads) are distinct types with
/// distinct on-disk shapes; this test feeds the SAME byte-split from the planner
/// into the real index (via `to_release_parts`), round-trips it through that
/// index's canonical parser ([`PackReleaseIndex::from_value`], the counterpart
/// to the [`PackReleaseIndex::save`] write path), and asserts every accounting
/// field — part count, per-part filename/bytes/sha256, `part_size`,
/// `total_bytes`, and the overall sha256 — is identical. If the two
/// representations ever diverge, this fails.
#[test]
fn planner_parts_match_the_pull_facing_release_index() {
    const PACK: &str = "cve";
    const PART_SIZE: u64 = 1024;
    let data = pseudo_random_bytes(4703, 0xC0FFEE);

    let plan = plan_multipart_release(&data, PART_SIZE).expect("plan release");
    assert!(plan.parts.len() > 1, "fixture must be multi-part");

    // Build the REAL pull-facing index from the planner's byte-split.
    let manifest = kgpacks_packs::validate_manifest(&serde_json::json!({
        "name": PACK,
        "version": "0.1.0",
    }))
    .expect("valid manifest");
    let release_parts = plan.to_release_parts(PACK);
    let index = build_release_index(
        &manifest,
        "cve-2025.06",
        &ProvenanceOverrides::default(),
        "2026-01-02T00:00:00Z",
        plan.sha256.clone(),
        plan.total_bytes,
        plan.part_size,
        release_parts,
    );

    // Round-trip through the on-disk index's canonical parser (the read
    // counterpart to `PackReleaseIndex::save`).
    let reparsed =
        PackReleaseIndex::from_value(&index.to_value()).expect("real index must validate");
    assert_eq!(reparsed, index, "index must survive its canonical parser");

    // Top-level accounting agrees between the planner and the real index.
    assert_eq!(reparsed.part_size, plan.part_size);
    assert_eq!(reparsed.total_bytes, plan.total_bytes);
    assert_eq!(reparsed.sha256, plan.sha256);
    assert_eq!(reparsed.parts.len(), plan.parts.len());

    // Every part agrees byte-for-byte: 0-based ordinal → `<pack>.tar.gz.NNN`
    // filename, identical length, identical per-part SHA-256.
    for (planned, real) in plan.parts.iter().zip(&reparsed.parts) {
        assert_eq!(real.file, pack_part_filename(PACK, planned.index));
        assert_eq!(real.bytes, planned.bytes);
        assert_eq!(real.sha256, planned.sha256);
    }

    // And the parts still reassemble to the original artifact + overall digest.
    let mut offset = 0usize;
    for real in &reparsed.parts {
        let slice = &data[offset..offset + real.bytes as usize];
        assert_eq!(real.sha256, sha256_hex(slice));
        offset += real.bytes as usize;
    }
    assert_eq!(offset, data.len());
    assert_eq!(reparsed.sha256, sha256_hex(&data));
}

#[test]
fn part_filenames_are_zero_padded_ordinals() {
    assert_eq!(pack_part_filename("cve", 0), "cve.tar.gz.000");
    assert_eq!(pack_part_filename("cve", 1), "cve.tar.gz.001");
    assert_eq!(pack_part_filename("cve", 42), "cve.tar.gz.042");
    assert_eq!(pack_part_filename("cve", 1234), "cve.tar.gz.1234");
}
