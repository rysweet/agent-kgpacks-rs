//! Contract tests for the `kgpacks-packs` release-index model + release-tag /
//! version / provenance derivation (WS3).
//!
//! Ports the pack-publish half of `scripts/release-pack.mjs` and
//! `packVersionFromReleaseTag`: provenance mirroring (manifest ↔
//! `<name>.pack-release.json`) round-trip, unpadded version derivation (valid +
//! rejection), and the dated-tag publish / latest-resolve pointer logic.

use std::collections::BTreeMap;

use kgpacks_packs::{
    build_release_index, build_release_provenance, iso8601_utc_from_unix, latest_release_tag,
    pack_release_filename, plan_release, publish_targets, resolve_model, resolve_release_version,
    PackManifest, PackReleaseIndex, ProvenanceOverrides, ReleasePart, LATEST_POINTER_TAG,
    RELEASE_INDEX_FORMAT,
};
use serde_json::{json, Value};

fn manifest_with_provenance(provenance: Value) -> PackManifest {
    let value = json!({
        "name": "cve",
        "version": "0.1.0",
        "provenance": provenance,
    });
    kgpacks_packs::validate_manifest(&value).expect("valid manifest")
}

fn full_provenance() -> Value {
    json!({
        "corpus": { "name": "cvelistV5", "commit": "abc123", "date": "2026-01-01" },
        "embedding": { "model": "Xenova/bge-base-en-v1.5", "dimensions": 768 },
        "build": { "date": "2026-01-02T00:00:00Z", "tool_version": "0.1.0" }
    })
}

// --- Publish targets: dated tag + stable `packs` latest-pointer. --------------

#[test]
fn publish_targets_appends_the_latest_pointer_for_a_dated_tag() {
    // A dated release publishes to its immutable tag AND moves the stable
    // `packs` latest-pointer so `pack pull` (which defaults to `packs`) resolves
    // the newest version.
    assert_eq!(
        publish_targets("cve-2025.06"),
        vec!["cve-2025.06".to_string(), LATEST_POINTER_TAG.to_string()]
    );
}

#[test]
fn publish_targets_to_the_pointer_itself_is_just_the_pointer() {
    assert_eq!(publish_targets("packs"), vec!["packs".to_string()]);
}

#[test]
fn resolve_release_version_uses_the_dated_tag_but_falls_back_for_the_pointer() {
    // Dated tag → derived unpadded SemVer.
    assert_eq!(resolve_release_version("cve-2025.06", "0.1.0"), "2025.6.0");
    // Non-dated pointer / invalid month → the manifest version.
    assert_eq!(resolve_release_version("packs", "0.1.0"), "0.1.0");
    assert_eq!(resolve_release_version("cve-2025.13", "0.1.0"), "0.1.0");
}

// --- Latest-resolve over a set of candidate tags (`pack pull`). ---------------

#[test]
fn latest_release_tag_picks_the_newest_dated_tag_ignoring_pointers() {
    let tags = [
        "cve-2024.12",
        "cve-2025.06",
        "packs",
        "cve-2025.06.1",
        "cve",
    ];
    // 2025.6.1 > 2025.6.0 > 2024.12.0; non-dated `packs`/`cve` are ignored.
    assert_eq!(latest_release_tag(&tags).unwrap(), Some("cve-2025.06.1"));
}

#[test]
fn latest_release_tag_is_none_without_a_dated_tag() {
    assert_eq!(
        latest_release_tag(&["packs", "cve", "latest"]).unwrap(),
        None
    );
    assert_eq!(latest_release_tag(&[]).unwrap(), None);
}

// --- Provenance mirror: manifest → release index (and round-trip). ------------

#[test]
fn build_release_provenance_mirrors_a_full_manifest_block_verbatim() {
    let manifest = manifest_with_provenance(full_provenance());
    // With every section fully populated (incl. build.date), nothing is
    // defaulted: the release provenance equals the manifest provenance.
    let provenance =
        build_release_provenance(&manifest, &ProvenanceOverrides::default(), "IGNORED").unwrap();
    assert_eq!(provenance, full_provenance());
}

#[test]
fn build_release_provenance_fills_gaps_from_overrides_and_build_date() {
    // Manifest provenance missing corpus commit/date, embedding model, and
    // build.date — all should be filled.
    let manifest = manifest_with_provenance(json!({
        "corpus": { "name": "cvelistV5" },
        "embedding": { "dimensions": 768 }
    }));
    let overrides = ProvenanceOverrides {
        corpus_commit: Some("deadbeef".into()),
        corpus_date: Some("2026-06-01".into()),
        model: Some("Xenova/bge-base-en-v1.5".into()),
    };
    let provenance =
        build_release_provenance(&manifest, &overrides, "2026-06-02T03:04:05Z").unwrap();
    assert_eq!(
        provenance,
        json!({
            "corpus": { "name": "cvelistV5", "commit": "deadbeef", "date": "2026-06-01" },
            "embedding": { "dimensions": 768, "model": "Xenova/bge-base-en-v1.5" },
            "build": { "date": "2026-06-02T03:04:05Z" }
        })
    );
}

#[test]
fn build_release_provenance_defaults_build_date_only_when_absent() {
    // A present build.date is preserved (not overwritten by `now_iso`).
    let manifest = manifest_with_provenance(json!({
        "build": { "date": "2020-01-01T00:00:00Z", "tool_version": "9.9.9" }
    }));
    let provenance = build_release_provenance(
        &manifest,
        &ProvenanceOverrides::default(),
        "2030-01-01T00:00:00Z",
    )
    .unwrap();
    assert_eq!(provenance["build"]["date"], "2020-01-01T00:00:00Z");
    assert_eq!(provenance["build"]["tool_version"], "9.9.9");
}

#[test]
fn build_release_provenance_is_none_when_there_is_nothing_to_record() {
    // No manifest provenance, no overrides, no model — but build.date is always
    // stamped, so the block still carries the build section.
    let manifest = PackManifest::new("cve", "0.1.0");
    let provenance = build_release_provenance(
        &manifest,
        &ProvenanceOverrides::default(),
        "2026-01-01T00:00:00Z",
    )
    .unwrap();
    assert_eq!(
        provenance,
        json!({ "build": { "date": "2026-01-01T00:00:00Z" } })
    );
}

#[test]
fn release_index_round_trips_with_mirrored_provenance() {
    let manifest = manifest_with_provenance(full_provenance());
    let parts = vec![
        ReleasePart {
            file: "cve.tar.gz.000".into(),
            bytes: 1_900 * 1024 * 1024,
            sha256: "aa".repeat(32),
        },
        ReleasePart {
            file: "cve.tar.gz.001".into(),
            bytes: 42,
            sha256: "bb".repeat(32),
        },
    ];
    let index = build_release_index(
        &manifest,
        "cve-2025.06",
        &ProvenanceOverrides::default(),
        "2026-01-02T00:00:00Z",
        "cc".repeat(32),
        1_900 * 1024 * 1024 + 42,
        1_900 * 1024 * 1024,
        parts,
    );

    // The index mirrors the manifest provenance and derives the dated version.
    assert_eq!(index.name, "cve");
    assert_eq!(index.version, "2025.6.0");
    assert_eq!(index.format, RELEASE_INDEX_FORMAT);
    assert_eq!(index.provenance.as_ref().unwrap(), &full_provenance());

    // Round-trip: serialize → parse → identical struct.
    let value = index.to_value();
    let reparsed = PackReleaseIndex::from_value(&value).expect("valid index");
    assert_eq!(reparsed, index);

    // On-disk camelCase keys match the reference index shape.
    assert_eq!(value["createdAt"], "2026-01-02T00:00:00Z");
    assert_eq!(value["totalBytes"], 1_900 * 1024 * 1024_u64 + 42);
    assert_eq!(value["partSize"], 1_900 * 1024 * 1024_u64);
    assert_eq!(value["parts"][0]["file"], "cve.tar.gz.000");
    assert_eq!(value["provenance"]["corpus"]["commit"], "abc123");
}

#[test]
fn release_index_from_value_rejects_a_malformed_provenance_block() {
    // A malformed provenance block in a release index is rejected through the
    // same gate a manifest uses (here: a declared string field is a number).
    let value = json!({
        "name": "cve",
        "version": "2025.6.0",
        "format": RELEASE_INDEX_FORMAT,
        "createdAt": "2026-01-02T00:00:00Z",
        "sha256": "cc",
        "totalBytes": 1,
        "partSize": 1,
        "parts": [],
        "provenance": { "corpus": { "commit": 123 } }
    });
    assert!(PackReleaseIndex::from_value(&value).is_err());
}

#[test]
fn release_index_from_value_requires_the_core_fields() {
    // Missing `parts` (and other required fields) is a validation error.
    let value = json!({ "name": "cve", "version": "2025.6.0" });
    assert!(PackReleaseIndex::from_value(&value).is_err());
}

#[test]
fn resolve_model_prefers_overrides_then_manifest_fields() {
    let mut extra = BTreeMap::new();
    extra.insert("model".to_string(), json!("from-manifest"));
    let manifest = PackManifest {
        extra,
        ..PackManifest::new("cve", "0.1.0")
    };
    // Override wins.
    assert_eq!(
        resolve_model(
            &manifest,
            &ProvenanceOverrides {
                model: Some("from-override".into()),
                ..Default::default()
            }
        ),
        Some("from-override".to_string())
    );
    // Else the manifest `model` field.
    assert_eq!(
        resolve_model(&manifest, &ProvenanceOverrides::default()),
        Some("from-manifest".to_string())
    );
    // Else `None`.
    assert_eq!(
        resolve_model(
            &PackManifest::new("cve", "0.1.0"),
            &ProvenanceOverrides::default()
        ),
        None
    );
}

// --- Offline release plan (CLI `pack release-plan` projection). ---------------

#[test]
fn plan_release_projects_version_targets_and_provenance() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let pack_dir = tmp.path().join("cve");
    std::fs::create_dir_all(&pack_dir).unwrap();
    let manifest = json!({
        "name": "cve",
        "version": "0.1.0",
        "provenance": full_provenance()
    });
    std::fs::write(
        pack_dir.join("manifest.json"),
        serde_json::to_string(&manifest).unwrap(),
    )
    .unwrap();

    let plan = plan_release(
        &pack_dir,
        "cve-2025.06",
        &ProvenanceOverrides::default(),
        "IGNORED",
    )
    .expect("plan");

    assert_eq!(plan.name, "cve");
    assert_eq!(plan.tag, "cve-2025.06");
    assert_eq!(plan.version, "2025.6.0");
    assert_eq!(
        plan.publish_targets,
        vec!["cve-2025.06".to_string(), "packs".to_string()]
    );
    assert_eq!(plan.index_filename, "cve.pack-release.json");
    assert_eq!(plan.provenance.as_ref().unwrap(), &full_provenance());
}

#[test]
fn pack_release_filename_is_name_scoped() {
    assert_eq!(pack_release_filename("cve"), "cve.pack-release.json");
}

// --- ISO-8601 timestamp helper (dependency-free `build.date`/`createdAt`). ----

#[test]
fn iso8601_utc_from_unix_formats_known_instants() {
    assert_eq!(iso8601_utc_from_unix(0), "1970-01-01T00:00:00Z");
    assert_eq!(iso8601_utc_from_unix(1_700_000_000), "2023-11-14T22:13:20Z");
    // Leap-year boundary.
    assert_eq!(iso8601_utc_from_unix(1_582_934_400), "2020-02-29T00:00:00Z");
}
